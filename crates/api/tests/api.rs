//! End-to-end tests of the real router against AWS Emulator.
//!
//! Run them with `make test-integration`. Each test creates its own table, bucket, and Cognito user pool.

mod support;

use axum::http::{Method, StatusCode};
use openbook_core::model::FakeChat;
use serde_json::{Value, json};
use support::{Options, TestApp, TestUser, send_to, start, start_with, start_with_models};

const IGNORE: &str = "needs AWS Emulator; run make test-integration";

fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("no string {key} in {value}"))
        .to_owned()
}

/// Sends a chat message and checks that it streams.
async fn ask(app: &TestApp, user: &TestUser, course_id: &str, body: Value) -> Vec<(String, Value)> {
    let reply = app.chat(user, course_id, body).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert_eq!(reply.content_type, "text/event-stream");
    reply.sse()
}

fn conversation_id(events: &[(String, Value)]) -> String {
    text(&events[0].1, "conversationId")
}

fn answer_text(events: &[(String, Value)]) -> String {
    events
        .iter()
        .filter(|(n, _)| n == "delta")
        .map(|(_, d)| text(d, "text"))
        .collect()
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn health_needs_no_token_and_unknown_routes_are_json() {
    let _ = IGNORE;
    let app = start().await;
    let reply = app.send(Method::GET, "/api/health", None, None).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({"ok": true, "stage": "local", "version": env!("CARGO_PKG_VERSION")})
    );

    let reply = app.send(Method::GET, "/api/nothing", None, None).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.error_code(), "not_found");

    // A wrong method is also a JSON error.
    let reply = app.send(Method::DELETE, "/api/health", None, None).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.error_code(), "not_found");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn me_creates_a_profile_and_reports_usage_and_limits() {
    let app = start_with(Options {
        user_daily_limit: 7,
        ..Options::default()
    })
    .await;
    let user = app.user("alice").await;
    let reply = app.get("/api/me", &user).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let body = reply.json();
    assert_eq!(body["user"], json!({"id": user.id, "email": user.email}));
    assert_eq!(body["usage"]["messagesUsed"], 0);
    assert_eq!(body["usage"]["messagesLimit"], 7);
    assert_eq!(body["limits"]["dailyMessages"], 7);
    assert_eq!(body["limits"]["maxCoursesOwned"], 10);
    assert_eq!(body["limits"]["maxDocumentsPerCourse"], 50);
    assert_eq!(body["limits"]["maxUploadBytes"], 25 * 1024 * 1024);
    // The profile item exists, and a second call keeps it.
    assert_eq!(app.items_under(&format!("USER#{}", user.id)).await, 1);
    assert_eq!(app.get("/api/me", &user).await.status, StatusCode::OK);
    assert_eq!(app.items_under(&format!("USER#{}", user.id)).await, 1);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn tokens_are_checked_on_every_route() {
    let app = start().await;
    let user = app.user("alice").await;

    let reply = app.send(Method::GET, "/api/me", None, None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.error_code(), "unauthenticated");

    let reply = app
        .send(Method::GET, "/api/me", Some("not-a-token"), None)
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);

    // Only x-auth-token counts. The Authorization header is never read.
    let request = axum::http::Request::builder()
        .uri("/api/me")
        .header("authorization", format!("Bearer {}", user.token))
        .body(axum::body::Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(app.router.clone(), request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // A token from another pool (another issuer and client) is refused.
    let other = start().await;
    let stranger = other.user("mallory").await;
    let reply = app
        .send(Method::GET, "/api/me", Some(&stranger.token), None)
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);

    for (method, path) in [
        (Method::GET, "/api/courses"),
        (Method::POST, "/api/courses"),
        (Method::POST, "/api/join"),
        (Method::GET, "/api/models"),
        (Method::GET, "/api/conversations/01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        (Method::POST, "/api/courses/01ARZ3NDEKTSV4RRFFQ69G5FAV/chat"),
    ] {
        let reply = app.send(method, path, None, Some(json!({}))).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn course_creation_makes_the_caller_the_owner() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let reply = app
        .post(
            "/api/courses",
            &owner,
            json!({"name": "  Linear Algebra ", "description": "Vectors", "tutoringStyle": "hints", "model": "fake-b"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    let course = reply.json()["course"].clone();
    assert_eq!(course["name"], "Linear Algebra");
    assert_eq!(course["description"], "Vectors");
    assert_eq!(course["tutoringStyle"], "hints");
    assert_eq!(course["model"], "fake-b");
    assert_eq!(course["role"], "owner");
    let code = text(&course, "inviteCode");
    assert_eq!(code.len(), 8);
    assert!(
        code.bytes()
            .all(|b| b"ABCDEFGHJKMNPQRSTUVWXYZ23456789".contains(&b))
    );
    let course_id = text(&course, "id");
    assert_eq!(course_id.len(), 26);
    assert!(course["createdAt"].as_str().unwrap().ends_with('Z'));

    // The course, the owner membership, and the invite code all exist.
    assert_eq!(app.items_under(&format!("COURSE#{course_id}")).await, 2);
    assert_eq!(app.items_under(&format!("INVITE#{code}")).await, 1);

    let listed = app.get("/api/courses", &owner).await.json();
    assert_eq!(listed["courses"].as_array().unwrap().len(), 1);
    assert_eq!(listed["courses"][0]["id"], course_id);
    assert_eq!(listed["courses"][0]["role"], "owner");
    let fetched = app
        .get(&format!("/api/courses/{course_id}"), &owner)
        .await
        .json();
    assert_eq!(fetched["course"], course);

    // Defaults.
    let plain = app.course(&owner, "Plain").await;
    assert_eq!(plain["tutoringStyle"], "socratic");
    assert_eq!(plain["description"], "");
    assert_eq!(plain["model"], "fake-a");
    // Newest membership first.
    let listed = app.get("/api/courses", &owner).await.json();
    assert_eq!(listed["courses"][0]["id"], plain["id"]);
    assert_eq!(listed["courses"][1]["id"], course["id"]);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn course_input_is_validated() {
    let app = start_with(Options {
        courses_owned: 2,
        ..Options::default()
    })
    .await;
    let owner = app.user("teacher").await;
    for body in [
        json!({"name": "   "}),
        json!({"name": "x".repeat(81)}),
        json!({"name": "ok", "description": "x".repeat(501)}),
        json!({"name": "ok", "tutoringStyle": "rude"}),
        json!({"name": "ok", "model": "gpt-9"}),
        json!({"description": "no name"}),
        json!("not an object"),
    ] {
        let reply = app.post("/api/courses", &owner, body.clone()).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            reply.text
        );
        assert_eq!(reply.error_code(), "invalid_request");
    }
    let reply = app
        .send(Method::POST, "/api/courses", Some(&owner.token), None)
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    app.course(&owner, "One").await;
    app.course(&owner, "Two").await;
    let reply = app
        .post("/api/courses", &owner, json!({"name": "Three"}))
        .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error_code(), "limit_reached");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn owner_updates_a_course() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "Old name").await;
    let path = format!("/api/courses/{}", text(&course, "id"));
    let reply = app
        .patch(&path, &owner, json!({"name": "New name", "tutoringStyle": "direct", "model": "fake-b", "description": "d"}))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let updated = reply.json()["course"].clone();
    assert_eq!(updated["name"], "New name");
    assert_eq!(updated["tutoringStyle"], "direct");
    assert_eq!(updated["model"], "fake-b");
    assert_eq!(updated["description"], "d");
    assert_eq!(updated["inviteCode"], course["inviteCode"]);
    assert_eq!(updated["createdAt"], course["createdAt"]);
    assert_ne!(updated["updatedAt"], course["updatedAt"]);

    // A partial patch leaves the other fields alone.
    let reply = app.patch(&path, &owner, json!({"name": "Third"})).await;
    assert_eq!(reply.json()["course"]["model"], "fake-b");

    let reply = app.patch(&path, &owner, json!({"model": "nope"})).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    let reply = app.patch(&path, &owner, json!({"name": ""})).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn models_route_lists_the_catalog() {
    let app = start().await;
    let user = app.user("alice").await;
    let reply = app.get("/api/models", &user).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({"models": [
            {"id": "fake-a", "label": "Fake A", "vendor": "openbook", "residency": "au", "isDefault": true},
            {"id": "fake-b", "label": "Fake B", "vendor": "openbook", "residency": "global", "isDefault": false},
        ]})
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn second_user_joins_with_an_invite_code() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course(&owner, "Calculus").await;
    let code = text(&course, "inviteCode");
    let course_id = text(&course, "id");

    // The code works in lower case with spaces and hyphens.
    let messy = format!(
        " {}-{} ",
        code[..4].to_lowercase(),
        code[4..].to_lowercase()
    );
    let reply = app
        .post("/api/join", &student, json!({"inviteCode": messy}))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let joined = reply.json()["course"].clone();
    assert_eq!(joined["id"], course_id);
    assert_eq!(joined["role"], "student");
    assert!(
        joined.get("inviteCode").is_none(),
        "students must not see the code"
    );

    // Joining again changes nothing.
    let again = app
        .post("/api/join", &student, json!({"inviteCode": code}))
        .await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json()["course"]["role"], "student");
    // The owner who uses their own code stays the owner.
    let own = app
        .post("/api/join", &owner, json!({"inviteCode": code}))
        .await;
    assert_eq!(own.json()["course"]["role"], "owner");

    let fetched = app
        .get(&format!("/api/courses/{course_id}"), &student)
        .await
        .json();
    assert_eq!(fetched["course"]["role"], "student");
    assert!(fetched["course"].get("inviteCode").is_none());
    let listed = app.get("/api/courses", &student).await.json();
    assert_eq!(listed["courses"].as_array().unwrap().len(), 1);

    let members = app
        .get(&format!("/api/courses/{course_id}/members"), &owner)
        .await
        .json();
    let members = members["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["role"], "owner");
    assert_eq!(members[0]["userId"], owner.id);
    assert_eq!(members[1]["role"], "student");
    assert_eq!(members[1]["email"], student.email);

    for bad in ["ZZZZZZZZ", "short", "ABCD0123", ""] {
        let reply = app
            .post("/api/join", &student, json!({"inviteCode": bad}))
            .await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{bad}");
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn non_members_get_404_on_every_course_route() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let outsider = app.user("outsider").await;
    let course = app.course(&owner, "Private").await;
    let id = text(&course, "id");
    let doc = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    let checks: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, format!("/api/courses/{id}"), None),
        (
            Method::PATCH,
            format!("/api/courses/{id}"),
            Some(json!({"name": "x"})),
        ),
        (Method::DELETE, format!("/api/courses/{id}"), None),
        (Method::POST, format!("/api/courses/{id}/invite-code"), None),
        (Method::GET, format!("/api/courses/{id}/members"), None),
        (
            Method::DELETE,
            format!("/api/courses/{id}/members/{}", owner.id),
            None,
        ),
        (Method::GET, format!("/api/courses/{id}/documents"), None),
        (
            Method::POST,
            format!("/api/courses/{id}/documents"),
            Some(json!({"filename": "a.pdf", "sizeBytes": 5, "contentType": "application/pdf"})),
        ),
        (
            Method::POST,
            format!("/api/courses/{id}/documents/{doc}/complete"),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/courses/{id}/documents/{doc}"),
            None,
        ),
        (
            Method::GET,
            format!("/api/courses/{id}/conversations"),
            None,
        ),
        (
            Method::POST,
            format!("/api/courses/{id}/chat"),
            Some(json!({"message": "hello"})),
        ),
        (Method::GET, format!("/api/courses/{id}/insights"), None),
    ];
    for (method, path, body) in checks {
        let reply = app
            .send(method.clone(), &path, Some(&outsider.token), body)
            .await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{method} {path}: {}",
            reply.text
        );
        assert_eq!(reply.error_code(), "not_found");
    }
    // A malformed ID is also 404, and the owner still has the course.
    assert_eq!(
        app.get("/api/courses/not-an-id", &outsider).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/courses/{id}"), &owner).await.status,
        StatusCode::OK
    );
    assert_eq!(
        app.get("/api/courses", &outsider).await.json()["courses"],
        json!([])
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn students_get_403_on_owner_routes() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");
    let doc = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    let checks: Vec<(Method, String, Option<Value>)> = vec![
        (
            Method::PATCH,
            format!("/api/courses/{id}"),
            Some(json!({"name": "x"})),
        ),
        (Method::DELETE, format!("/api/courses/{id}"), None),
        (Method::POST, format!("/api/courses/{id}/invite-code"), None),
        (Method::GET, format!("/api/courses/{id}/members"), None),
        (
            Method::DELETE,
            format!("/api/courses/{id}/members/{}", owner.id),
            None,
        ),
        (
            Method::POST,
            format!("/api/courses/{id}/documents"),
            Some(json!({"filename": "a.pdf", "sizeBytes": 5, "contentType": "application/pdf"})),
        ),
        (
            Method::POST,
            format!("/api/courses/{id}/documents/{doc}/complete"),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/courses/{id}/documents/{doc}"),
            None,
        ),
        (Method::GET, format!("/api/courses/{id}/insights"), None),
    ];
    for (method, path, body) in checks {
        let reply = app
            .send(method.clone(), &path, Some(&student.token), body)
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{method} {path}: {}",
            reply.text
        );
        assert_eq!(reply.error_code(), "forbidden");
    }
    // The course is untouched and students still read the routes that they may read.
    assert_eq!(
        app.get(&format!("/api/courses/{id}"), &owner).await.json()["course"]["name"],
        "Calculus"
    );
    assert_eq!(
        app.get(&format!("/api/courses/{id}/documents"), &student)
            .await
            .json()["documents"],
        json!([])
    );
    assert_eq!(
        app.get(&format!("/api/courses/{id}/conversations"), &student)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn rotating_the_invite_code_invalidates_the_old_one() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let late = app.user("late").await;
    let course = app.course(&owner, "Rotating").await;
    let old = text(&course, "inviteCode");
    let id = text(&course, "id");

    let reply = app
        .post_empty(&format!("/api/courses/{id}/invite-code"), &owner)
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let new = text(&reply.json(), "inviteCode");
    assert_ne!(new, old);
    assert_eq!(new.len(), 8);

    let reply = app
        .post("/api/join", &student, json!({"inviteCode": old}))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(app.items_under(&format!("INVITE#{old}")).await, 0);
    assert_eq!(app.items_under(&format!("INVITE#{new}")).await, 1);
    let reply = app
        .post("/api/join", &late, json!({"inviteCode": new}))
        .await;
    assert_eq!(reply.status, StatusCode::OK);

    // The owner sees the new code.
    let fetched = app.get(&format!("/api/courses/{id}"), &owner).await.json();
    assert_eq!(fetched["course"]["inviteCode"], new);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn upload_flow_presign_put_complete_and_delete() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");
    let bytes = b"%PDF-1.4 a tiny test file".to_vec();

    let reply = app
        .post(
            &format!("/api/courses/{id}/documents"),
            &owner,
            json!({"filename": "Lecture 1.PDF", "sizeBytes": bytes.len(), "contentType": "application/pdf"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    let body = reply.json();
    let document = body["document"].clone();
    let doc_id = text(&document, "id");
    assert_eq!(document["status"], "uploading");
    assert_eq!(document["filename"], "Lecture 1.PDF");
    assert_eq!(document["courseId"], id);
    assert_eq!(body["upload"]["method"], "PUT");
    assert_eq!(
        body["upload"]["headers"],
        json!({"content-type": "application/pdf"})
    );
    assert!(body["upload"]["expiresAt"].as_str().unwrap().ends_with('Z'));
    let url = text(&body["upload"], "url");
    assert!(url.contains(&format!("courses/{id}/documents/{doc_id}/source.pdf")));

    // The upload is not done yet, so complete says 409. Students do not see the document.
    let complete_path = format!("/api/courses/{id}/documents/{doc_id}/complete");
    let reply = app.post_empty(&complete_path, &owner).await;
    assert_eq!(reply.status, StatusCode::CONFLICT, "{}", reply.text);
    assert_eq!(reply.error_code(), "conflict");
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &owner)
        .await
        .json();
    assert_eq!(docs["documents"][0]["status"], "uploading");
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &student)
        .await
        .json();
    assert_eq!(docs["documents"], json!([]));

    // A real PUT to the presigned URL, with exactly the documented headers.
    let put = app
        .http
        .put(&url)
        .header("content-type", "application/pdf")
        .body(bytes.clone())
        .send()
        .await
        .expect("PUT to the presigned URL");
    assert!(
        put.status().is_success(),
        "PUT failed: {} {}",
        put.status(),
        put.text().await.unwrap_or_default()
    );

    let reply = app.post_empty(&complete_path, &owner).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert_eq!(reply.json()["document"]["status"], "queued");
    // Calling it again returns the document unchanged.
    let again = app.post_empty(&complete_path, &owner).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json()["document"]["status"], "queued");
    assert_eq!(again.json()["document"]["id"], doc_id);

    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &owner)
        .await
        .json();
    assert_eq!(docs["documents"].as_array().unwrap().len(), 1);
    assert_eq!(docs["documents"][0]["status"], "queued");
    // Students see only ready documents.
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &student)
        .await
        .json();
    assert_eq!(docs["documents"], json!([]));

    // The ingest message went to the queue.
    let messages = app.queued_messages().await;
    assert_eq!(
        messages,
        vec![json!({"action": "ingest", "courseId": id, "documentId": doc_id})]
    );

    // Delete sets `deleting` and sends the `delete` message. The worker does the rest.
    let reply = app
        .delete(&format!("/api/courses/{id}/documents/{doc_id}"), &owner)
        .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text);
    assert_eq!(reply.json()["document"]["status"], "deleting");
    assert_eq!(reply.json()["document"]["id"], doc_id);
    let again = app
        .delete(&format!("/api/courses/{id}/documents/{doc_id}"), &owner)
        .await;
    assert_eq!(again.status, StatusCode::ACCEPTED);
    assert_eq!(again.json()["document"]["status"], "deleting");
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &owner)
        .await
        .json();
    assert_eq!(docs["documents"][0]["status"], "deleting");
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &student)
        .await
        .json();
    assert_eq!(docs["documents"], json!([]));
    let messages = app.queued_messages().await;
    assert_eq!(
        messages,
        vec![json!({"action": "delete", "courseId": id, "documentId": doc_id})],
        "one delete message, and none for the repeated call"
    );
    // The object stays until the worker runs.
    assert_eq!(
        app.objects_under(&format!("courses/{id}/documents/{doc_id}/"))
            .await
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_failed_queue_send_rolls_back_complete_and_delete() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "Queue").await;
    let id = text(&course, "id");
    let reply = app
        .post(
            &format!("/api/courses/{id}/documents"),
            &owner,
            json!({"filename": "a.pdf", "sizeBytes": 3, "contentType": "application/pdf"}),
        )
        .await;
    let doc_id = text(&reply.json()["document"], "id");
    let put = app
        .http
        .put(text(&reply.json()["upload"], "url"))
        .header("content-type", "application/pdf")
        .body(vec![1u8, 2, 3])
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success());

    // The same data, behind a queue that does not exist.
    let broken = app.router_with_queue("https://127.0.0.1:9/queue-that-is-gone.fifo");
    let complete = format!("/api/courses/{id}/documents/{doc_id}/complete");
    let reply = send_to(&broken, Method::POST, &complete, Some(&owner.token), None).await;
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.text
    );
    assert_eq!(reply.error_code(), "unavailable");
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &owner)
        .await
        .json();
    assert_eq!(docs["documents"][0]["status"], "uploading", "rolled back");
    assert!(app.queued_messages().await.is_empty());

    // The client can call `complete` again.
    let ok = app.post_empty(&complete, &owner).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.json()["document"]["status"], "queued");

    // A failed delete send restores the status.
    let path = format!("/api/courses/{id}/documents/{doc_id}");
    let reply = send_to(&broken, Method::DELETE, &path, Some(&owner.token), None).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &owner)
        .await
        .json();
    assert_eq!(docs["documents"][0]["status"], "queued");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn complete_without_an_upload_or_with_a_wrong_size_gives_409() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "Uploads").await;
    let id = text(&course, "id");
    let reply = app
        .post(
            &format!("/api/courses/{id}/documents"),
            &owner,
            json!({"filename": "a.pdf", "sizeBytes": 100, "contentType": "application/pdf"}),
        )
        .await;
    let doc_id = text(&reply.json()["document"], "id");
    let complete_path = format!("/api/courses/{id}/documents/{doc_id}/complete");

    let reply = app.post_empty(&complete_path, &owner).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error_code(), "conflict");

    // An object of another size does not pass either.
    app.s3
        .put_object()
        .bucket(&app.bucket)
        .key(format!("courses/{id}/documents/{doc_id}/source.pdf"))
        .body(vec![0u8; 10].into())
        .send()
        .await
        .expect("put object");
    let reply = app.post_empty(&complete_path, &owner).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    let docs = app
        .get(&format!("/api/courses/{id}/documents"), &owner)
        .await
        .json();
    assert_eq!(docs["documents"][0]["status"], "uploading");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn document_input_is_validated_and_limited() {
    let app = start_with(Options {
        documents_per_course: 1,
        upload_bytes: 1000,
        ..Options::default()
    })
    .await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "Limits").await;
    let path = format!("/api/courses/{}/documents", text(&course, "id"));

    for body in [
        json!({"filename": "notes.txt", "sizeBytes": 10, "contentType": "application/pdf"}),
        json!({"filename": "", "sizeBytes": 10, "contentType": "application/pdf"}),
        json!({"filename": "a.pdf", "sizeBytes": 0, "contentType": "application/pdf"}),
        json!({"filename": "a.pdf", "sizeBytes": -5, "contentType": "application/pdf"}),
        json!({"filename": "a.pdf", "sizeBytes": 10, "contentType": "image/png"}),
        json!({"filename": "a.pdf", "contentType": "application/pdf"}),
    ] {
        let reply = app.post(&path, &owner, body.clone()).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            reply.text
        );
    }
    let reply = app
        .post(
            &path,
            &owner,
            json!({"filename": "a.pdf", "sizeBytes": 1001, "contentType": "application/pdf"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.error_code(), "too_large");

    let ok = json!({"filename": "a.pdf", "sizeBytes": 1000, "contentType": "application/pdf"});
    assert_eq!(
        app.post(&path, &owner, ok.clone()).await.status,
        StatusCode::CREATED
    );
    let reply = app.post(&path, &owner, ok).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error_code(), "limit_reached");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn chat_streams_meta_then_deltas_then_done_and_saves_both_messages() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");

    let question = "What is the derivative of x squared?";
    let events = ask(
        &app,
        &student,
        &id,
        json!({"message": format!("  {question}  ")}),
    )
    .await;
    let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names.first(), Some(&"meta"));
    assert_eq!(names.last(), Some(&"done"));
    assert!(names[1..names.len() - 1].iter().all(|n| *n == "delta"));
    assert!(names.iter().filter(|n| **n == "delta").count() > 3);

    let meta = &events[0].1;
    let conv_id = text(meta, "conversationId");
    assert_eq!(conv_id.len(), 26);
    assert_eq!(meta["title"], question);
    assert_eq!(meta["model"], "fake-a");
    let user_message_id = text(meta, "userMessageId");
    let assistant_message_id = text(meta, "assistantMessageId");
    assert!(user_message_id < assistant_message_id);

    let answer = answer_text(&events);
    assert_eq!(answer, FakeChat::answer("fake-a", question));
    let done = &events.last().unwrap().1;
    assert_eq!(done["stopReason"], "end_turn");
    assert!(done["usage"]["inputTokens"].as_u64().unwrap() > 0);
    assert_eq!(
        done["usage"]["outputTokens"],
        answer.chars().count().div_ceil(4)
    );

    // Both messages are saved, oldest first.
    let reply = app
        .get(&format!("/api/conversations/{conv_id}"), &student)
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let body = reply.json();
    assert_eq!(body["conversation"]["id"], conv_id);
    assert_eq!(body["conversation"]["courseId"], id);
    assert_eq!(body["conversation"]["model"], "fake-a");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], user_message_id);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], question);
    assert!(messages[0].get("model").is_none());
    assert_eq!(messages[1]["id"], assistant_message_id);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"], answer);
    assert_eq!(messages[1]["model"], "fake-a");

    // The conversation is listed for the student only.
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &student)
        .await
        .json();
    assert_eq!(listed["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(listed["conversations"][0]["id"], conv_id);
    assert_eq!(listed["conversations"][0]["title"], question);
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &owner)
        .await
        .json();
    assert_eq!(listed["conversations"], json!([]));
    assert_eq!(
        app.get(&format!("/api/conversations/{conv_id}"), &owner)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // A student's message is a question for the owner. The owner's own message is not.
    let events = ask(&app, &owner, &id, json!({"message": "Owner test question"})).await;
    assert_eq!(events.last().unwrap().0, "done");
    let insights = app
        .get(&format!("/api/courses/{id}/insights"), &owner)
        .await
        .json();
    let questions = insights["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 1, "{insights}");
    assert_eq!(questions[0]["question"], question);
    assert_eq!(questions[0]["userId"], student.id);
    assert_eq!(questions[0]["email"], student.email);
    assert_eq!(questions[0]["conversationId"], conv_id);
    assert_eq!(
        insights["totals"],
        json!({"questionsLast7Days": 1, "activeStudentsLast7Days": 1})
    );

    // Usage counts both users' messages, and the token totals are recorded.
    let me = app.get("/api/me", &student).await.json();
    assert_eq!(me["usage"]["messagesUsed"], 1);
    let usage_items = app.items_under(&format!("USER#{}", student.id)).await;
    assert_eq!(
        usage_items, 4,
        "conversation, daily usage, monthly usage, and no profile yet"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn chat_continues_a_conversation_with_history_and_keeps_it_sorted() {
    let app = start().await;
    let user = app.user("teacher").await;
    let course = app.course(&user, "History").await;
    let id = text(&course, "id");

    let first = ask(
        &app,
        &user,
        &id,
        json!({"message": "First question about limits"}),
    )
    .await;
    let first_id = conversation_id(&first);
    let second = ask(&app, &user, &id, json!({"message": "A different topic"})).await;
    let second_id = conversation_id(&second);
    assert_ne!(first_id, second_id);

    // The most recently updated conversation is first.
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &user)
        .await
        .json();
    assert_eq!(listed["conversations"][0]["id"], second_id);
    assert_eq!(listed["conversations"][1]["id"], first_id);

    // A follow-up in the first conversation moves it to the front and adds two messages.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let follow = ask(
        &app,
        &user,
        &id,
        json!({"conversationId": first_id, "message": "And the next step?"}),
    )
    .await;
    assert_eq!(conversation_id(&follow), first_id);
    assert_eq!(follow[0].1["title"], "First question about limits");
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &user)
        .await
        .json();
    assert_eq!(listed["conversations"][0]["id"], first_id);
    assert_eq!(listed["conversations"].as_array().unwrap().len(), 2);
    let body = app
        .get(&format!("/api/conversations/{first_id}"), &user)
        .await
        .json();
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);

    // Another conversation or course cannot be used.
    let other_course = app.course(&user, "Other").await;
    let reply = app
        .chat(
            &user,
            &text(&other_course, "id"),
            json!({"conversationId": first_id, "message": "wrong course"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let reply = app
        .chat(
            &user,
            &id,
            json!({"conversationId": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "message": "x"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let reply = app
        .chat(&user, &id, json!({"conversationId": "bad", "message": "x"}))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);

    // Conversations are private and can be deleted.
    let other = app.user("other").await;
    assert_eq!(
        app.get(&format!("/api/conversations/{first_id}"), &other)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.delete(&format!("/api/conversations/{first_id}"), &other)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.delete(&format!("/api/conversations/{first_id}"), &user)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&format!("/api/conversations/{first_id}"), &user)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.items_under(&format!("CONV#{first_id}")).await, 0);
    assert_eq!(
        app.get(&format!("/api/conversations/{second_id}"), &user)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn chat_input_is_validated() {
    let app = start().await;
    let user = app.user("teacher").await;
    let course = app.course(&user, "Validation").await;
    let id = text(&course, "id");
    for body in [
        json!({"message": "   "}),
        json!({"message": "x".repeat(4001)}),
        json!({}),
        json!({"message": "hi", "model": "gpt-9"}),
        json!({"message": 5}),
    ] {
        let reply = app.chat(&user, &id, body.clone()).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            reply.text
        );
        assert_eq!(reply.error_code(), "invalid_request");
        assert_eq!(reply.content_type, "application/json");
    }
    // Nothing was reserved or saved.
    assert_eq!(
        app.get("/api/me", &user).await.json()["usage"]["messagesUsed"],
        0
    );
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &user)
        .await
        .json();
    assert_eq!(listed["conversations"], json!([]));
    // 4000 characters are fine.
    assert_eq!(
        app.chat(&user, &id, json!({"message": "x".repeat(4000)}))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn chat_model_choice_follows_request_then_conversation_then_course() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");

    // The course default is fake-a.
    let events = ask(&app, &student, &id, json!({"message": "one"})).await;
    assert_eq!(events[0].1["model"], "fake-a");
    assert!(answer_text(&events).contains("`fake-a`"));
    let conv = conversation_id(&events);

    // Any member can pick any catalog model. The conversation keeps it.
    let events = ask(
        &app,
        &student,
        &id,
        json!({"conversationId": conv, "message": "two", "model": "fake-b"}),
    )
    .await;
    assert_eq!(events[0].1["model"], "fake-b");
    assert!(answer_text(&events).contains("`fake-b`"));
    let events = ask(
        &app,
        &student,
        &id,
        json!({"conversationId": conv, "message": "three"}),
    )
    .await;
    assert_eq!(
        events[0].1["model"], "fake-b",
        "the conversation keeps its last model"
    );
    let body = app
        .get(&format!("/api/conversations/{conv}"), &student)
        .await
        .json();
    assert_eq!(body["conversation"]["model"], "fake-b");
    let models: Vec<Value> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].clone())
        .collect();
    assert_eq!(
        models,
        [
            Value::Null,
            json!("fake-a"),
            Value::Null,
            json!("fake-b"),
            Value::Null,
            json!("fake-b")
        ]
    );

    // A new conversation starts from the course default, and a request can override it at the start.
    let events = ask(
        &app,
        &student,
        &id,
        json!({"message": "new one", "model": "fake-b"}),
    )
    .await;
    assert_eq!(events[0].1["model"], "fake-b");
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &student)
        .await
        .json();
    assert_eq!(listed["conversations"][0]["model"], "fake-b");
    assert_eq!(listed["conversations"][1]["model"], "fake-b");

    // The owner changes the course default.
    let patch = app
        .patch(
            &format!("/api/courses/{id}"),
            &owner,
            json!({"model": "fake-b"}),
        )
        .await;
    assert_eq!(patch.status, StatusCode::OK);
    let events = ask(&app, &student, &id, json!({"message": "fresh"})).await;
    assert_eq!(events[0].1["model"], "fake-b");
    let seen = app
        .get(&format!("/api/courses/{id}"), &student)
        .await
        .json();
    assert_eq!(seen["course"]["model"], "fake-b");

    // Unknown models are refused before the stream starts.
    let reply = app
        .chat(
            &student,
            &id,
            json!({"message": "x", "model": "claude-opus"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_model_that_left_the_catalog_falls_back_to_the_default() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let reply = app
        .post(
            "/api/courses",
            &owner,
            json!({"name": "Old model", "model": "fake-b"}),
        )
        .await;
    let course = reply.json()["course"].clone();
    let id = text(&course, "id");
    let events = ask(&app, &owner, &id, json!({"message": "hello"})).await;
    let conv = conversation_id(&events);
    assert_eq!(events[0].1["model"], "fake-b");

    // The same data, behind a catalog without fake-b.
    let router = app.router_with_models(support::ONE_MODEL, "fake-a");
    let reply = send_to(
        &router,
        Method::GET,
        &format!("/api/courses/{id}"),
        Some(&owner.token),
        None,
    )
    .await;
    assert_eq!(reply.json()["course"]["model"], "fake-a");
    let reply = send_to(
        &router,
        Method::GET,
        &format!("/api/conversations/{conv}"),
        Some(&owner.token),
        None,
    )
    .await;
    assert_eq!(reply.json()["conversation"]["model"], "fake-a");
    let reply = send_to(
        &router,
        Method::POST,
        &format!("/api/courses/{id}/chat"),
        Some(&owner.token),
        Some(json!({"conversationId": conv, "message": "again"})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert_eq!(reply.sse()[0].1["model"], "fake-a");
    let reply = send_to(
        &router,
        Method::POST,
        &format!("/api/courses/{id}/chat"),
        Some(&owner.token),
        Some(json!({"message": "x", "model": "fake-b"})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn per_user_quota_gives_429_and_makes_no_model_call() {
    let app = start_with(Options {
        user_daily_limit: 2,
        ..Options::default()
    })
    .await;
    let user = app.user("student").await;
    let other = app.user("other").await;
    let course = app.course(&user, "Quota").await;
    let id = text(&course, "id");
    let joined = app
        .post(
            "/api/join",
            &other,
            json!({"inviteCode": course["inviteCode"]}),
        )
        .await;
    assert_eq!(joined.status, StatusCode::OK);

    let first = ask(&app, &user, &id, json!({"message": "one"})).await;
    let conv = conversation_id(&first);
    ask(
        &app,
        &user,
        &id,
        json!({"conversationId": conv, "message": "two"}),
    )
    .await;

    let reply = app
        .chat(
            &user,
            &id,
            json!({"conversationId": conv, "message": "three"}),
        )
        .await;
    assert_eq!(
        reply.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        reply.text
    );
    assert_eq!(reply.error_code(), "quota_exceeded");
    assert_eq!(reply.content_type, "application/json");

    // No model call: the third message was not saved and there is no extra answer.
    let body = app
        .get(&format!("/api/conversations/{conv}"), &user)
        .await
        .json();
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);
    let me = app.get("/api/me", &user).await.json();
    assert_eq!(me["usage"]["messagesUsed"], 2);
    assert_eq!(me["usage"]["messagesLimit"], 2);

    // The limit is per user.
    let events = ask(&app, &other, &id, json!({"message": "my turn"})).await;
    assert_eq!(events.last().unwrap().0, "done");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn global_cap_gives_503_and_returns_the_user_reservation() {
    let app = start_with(Options {
        global_daily_limit: 1,
        ..Options::default()
    })
    .await;
    let first = app.user("first").await;
    let second = app.user("second").await;
    let course = app.course(&first, "Cap").await;
    let id = text(&course, "id");
    app.post(
        "/api/join",
        &second,
        json!({"inviteCode": course["inviteCode"]}),
    )
    .await;

    let events = ask(&app, &first, &id, json!({"message": "I am first"})).await;
    assert_eq!(events.last().unwrap().0, "done");

    let reply = app
        .chat(&second, &id, json!({"message": "I am second"}))
        .await;
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.text
    );
    assert_eq!(reply.error_code(), "capacity_exhausted");
    assert_eq!(reply.content_type, "application/json");

    // The second user's reservation went back, and nothing of their message was saved.
    let me = app.get("/api/me", &second).await.json();
    assert_eq!(me["usage"]["messagesUsed"], 0);
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &second)
        .await
        .json();
    assert_eq!(listed["conversations"], json!([]));
    let insights = app
        .get(&format!("/api/courses/{id}/insights"), &first)
        .await
        .json();
    assert_eq!(insights["questions"], json!([]));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn deleting_a_course_removes_everything_that_belongs_to_it() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");
    let code = text(&course, "inviteCode");

    // A document with an object, and a student question.
    let reply = app
        .post(
            &format!("/api/courses/{id}/documents"),
            &owner,
            json!({"filename": "a.pdf", "sizeBytes": 3, "contentType": "application/pdf"}),
        )
        .await;
    let url = text(&reply.json()["upload"], "url");
    let put = app
        .http
        .put(&url)
        .header("content-type", "application/pdf")
        .body(vec![1u8, 2, 3])
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success());
    let events = ask(
        &app,
        &student,
        &id,
        json!({"message": "keep my conversation"}),
    )
    .await;
    let conv = conversation_id(&events);
    assert!(app.items_under(&format!("COURSE#{id}")).await >= 5);
    assert!(
        !app.objects_under(&format!("courses/{id}/"))
            .await
            .is_empty()
    );

    // Many extra items, so the delete has to batch.
    for n in 0..30 {
        let question = openbook_core::domain::Question {
            id: openbook_core::ids::new_id(),
            user_id: student.id.clone(),
            email: student.email.clone(),
            question: format!("extra {n}"),
            conversation_id: conv.clone(),
            created_at: openbook_core::domain::now(),
        };
        app.state.store.add_question(&id, &question).await.unwrap();
    }

    let reply = app.delete(&format!("/api/courses/{id}"), &owner).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text);

    // Only the question items stay. They expire by TTL and nothing can reach them.
    assert_eq!(app.items_under(&format!("COURSE#{id}")).await, 31);
    assert!(
        app.get_item_sk_prefixes(&format!("COURSE#{id}"))
            .await
            .iter()
            .all(|sk| sk.starts_with("Q#"))
    );
    assert_eq!(app.items_under(&format!("INVITE#{code}")).await, 0);
    assert!(
        app.objects_under(&format!("courses/{id}/"))
            .await
            .is_empty()
    );
    for user in [&owner, &student] {
        assert_eq!(
            app.get(&format!("/api/courses/{id}"), user).await.status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.get("/api/courses", user).await.json()["courses"],
            json!([])
        );
    }
    let reply = app
        .post("/api/join", &student, json!({"inviteCode": code}))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    // Conversations stay with their owners.
    let reply = app
        .get(&format!("/api/conversations/{conv}"), &student)
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn members_can_leave_and_owners_remove_students_but_the_owner_stays() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let leaver = app.user("leaver").await;
    let removed = app.user("removed").await;
    let course = app.course_with_student(&owner, &leaver).await;
    let id = text(&course, "id");
    app.post(
        "/api/join",
        &removed,
        json!({"inviteCode": course["inviteCode"]}),
    )
    .await;
    let member_path = |user: &TestUser| format!("/api/courses/{id}/members/{}", user.id);

    // A student cannot remove another student.
    let reply = app.delete(&member_path(&removed), &leaver).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    // Nobody can remove the owner, not even the owner.
    let reply = app.delete(&member_path(&owner), &owner).await;
    assert_eq!(reply.status, StatusCode::CONFLICT, "{}", reply.text);
    assert_eq!(reply.error_code(), "conflict");
    let reply = app.delete(&member_path(&owner), &leaver).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);

    // A student leaves.
    assert_eq!(
        app.delete(&member_path(&leaver), &leaver).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&format!("/api/courses/{id}"), &leaver).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/api/courses", &leaver).await.json()["courses"],
        json!([])
    );

    // The owner removes a student. Removing someone who is not a member is 404.
    assert_eq!(
        app.delete(&member_path(&removed), &owner).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.delete(&member_path(&removed), &owner).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/courses/{id}"), &removed)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let members = app
        .get(&format!("/api/courses/{id}/members"), &owner)
        .await
        .json();
    assert_eq!(members["members"].as_array().unwrap().len(), 1);
    assert_eq!(members["members"][0]["role"], "owner");

    // Someone who left can join again with the code.
    let rejoin = app
        .post(
            "/api/join",
            &leaver,
            json!({"inviteCode": course["inviteCode"]}),
        )
        .await;
    assert_eq!(rejoin.status, StatusCode::OK);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_catalog_with_only_one_model_serves_chat() {
    let app = start_with_models(Options::default(), support::ONE_MODEL).await;
    let user = app.user("solo").await;
    let course = app.course(&user, "Solo").await;
    let events = ask(&app, &user, &text(&course, "id"), json!({"message": "hi"})).await;
    assert_eq!(events[0].1["model"], "fake-a");
}
