//! Failure modes, limits under parallel requests, and request-level behavior.
//! They run the real router against AWS Emulator. Run them with `make test-integration`.

mod support;

use std::time::Duration;

use aws_sdk_dynamodb::types::AttributeValue;
use axum::http::{Method, StatusCode};
use futures::future::join_all;
use openbook_api::routes::chat::ChatTimeouts;
use serde_json::{Value, json};
use support::{Options, TestApp, TestUser, start, start_with};

fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("no string {key} in {value}"))
        .to_owned()
}

fn names(events: &[(String, Value)]) -> Vec<&str> {
    events.iter().map(|(n, _)| n.as_str()).collect()
}

async fn ask(app: &TestApp, user: &TestUser, course_id: &str, body: Value) -> Vec<(String, Value)> {
    let reply = app.chat(user, course_id, body).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    reply.sse()
}

async fn used(app: &TestApp, user: &TestUser) -> u64 {
    app.get("/api/me", user).await.json()["usage"]["messagesUsed"]
        .as_u64()
        .unwrap()
}

async fn item(
    app: &TestApp,
    pk: &str,
    sk: &str,
) -> Option<std::collections::HashMap<String, AttributeValue>> {
    app.dynamodb
        .get_item()
        .table_name(&app.table)
        .key("pk", AttributeValue::S(pk.to_owned()))
        .key("sk", AttributeValue::S(sk.to_owned()))
        .consistent_read(true)
        .send()
        .await
        .expect("get item")
        .item
}

fn number(item: &std::collections::HashMap<String, AttributeValue>, name: &str) -> i64 {
    match item.get(name) {
        Some(AttributeValue::N(v)) => v.parse().unwrap(),
        other => panic!("no number {name}: {other:?}"),
    }
}

fn month() -> String {
    openbook_core::quota::usage_month(chrono::Utc::now())
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn only_tokens_with_a_verified_email_are_accepted() {
    let app = start().await;
    let unverified = app.unverified_user("unverified").await;
    let reply = app.get("/api/me", &unverified).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text);
    assert_eq!(reply.error_code(), "unauthenticated");
    let verified = app.user("verified").await;
    assert_eq!(app.get("/api/me", &verified).await.status, StatusCode::OK);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_model_that_fails_before_the_first_delta_gives_the_reservation_back() {
    let app = start().await;
    let user = app.user("student").await;
    let course = app.course(&user, "Failures").await;
    let id = text(&course, "id");

    for marker in ["[fake:fail-before-delta]", "[fake:fail-start]"] {
        let events = ask(
            &app,
            &user,
            &id,
            json!({"message": format!("question {marker}")}),
        )
        .await;
        assert_eq!(names(&events), ["meta", "error"], "{marker}");
        assert_eq!(events[1].1["code"], "model_unavailable");
        assert!(!events[1].1["message"].as_str().unwrap().is_empty());
        assert_eq!(
            used(&app, &user).await,
            0,
            "{marker}: the reservation must go back"
        );
    }
    // The user message was saved, and no answer was.
    let listed = app
        .get(&format!("/api/courses/{id}/conversations"), &user)
        .await
        .json();
    let conv = text(&listed["conversations"][0], "id");
    let body = app
        .get(&format!("/api/conversations/{conv}"), &user)
        .await
        .json();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    // Nothing was counted for the month either.
    assert!(
        item(
            &app,
            &format!("USER#{}", user.id),
            &format!("USAGE#M#{}", month())
        )
        .await
        .is_none()
    );

    // The next question works, and the history has no two user turns in a row.
    let events = ask(
        &app,
        &user,
        &id,
        json!({"conversationId": conv, "message": "a good question"}),
    )
    .await;
    assert_eq!(events.last().unwrap().0, "done");
    assert_eq!(used(&app, &user).await, 1);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_model_that_fails_after_the_first_delta_still_counts_and_records_tokens() {
    let app = start().await;
    let user = app.user("student").await;
    let course = app.course(&user, "Failures").await;
    let id = text(&course, "id");

    let events = ask(
        &app,
        &user,
        &id,
        json!({"message": "half an answer [fake:fail-after-delta]"}),
    )
    .await;
    assert_eq!(names(&events), ["meta", "delta", "delta", "error"]);
    assert_eq!(events[3].1["code"], "model_unavailable");
    // The model produced text, so the message counts and the tokens are recorded.
    assert_eq!(used(&app, &user).await, 1);
    let monthly = item(
        &app,
        &format!("USER#{}", user.id),
        &format!("USAGE#M#{}", month()),
    )
    .await
    .expect("monthly usage item");
    assert_eq!(number(&monthly, "messages"), 1);
    assert!(number(&monthly, "inputTokens") > 0);
    assert!(number(&monthly, "outputTokens") > 0);
    // No partial answer is saved.
    let conv = text(&events[0].1, "conversationId");
    let body = app
        .get(&format!("/api/conversations/{conv}"), &user)
        .await
        .json();
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn stalled_models_time_out_with_an_error_event() {
    let timeouts = ChatTimeouts {
        overall: Duration::from_secs(5),
        first_delta: Duration::from_millis(300),
        gap: Duration::from_millis(300),
    };
    let app = start_with(Options {
        chat_timeouts: Some(timeouts),
        ..Options::default()
    })
    .await;
    let user = app.user("student").await;
    let course = app.course(&user, "Timeouts").await;
    let id = text(&course, "id");

    // No first delta: the message does not count.
    let events = ask(
        &app,
        &user,
        &id,
        json!({"message": "[fake:stall-before-delta]"}),
    )
    .await;
    assert_eq!(names(&events), ["meta", "error"]);
    assert_eq!(events[1].1["code"], "model_unavailable");
    assert_eq!(used(&app, &user).await, 0);

    // A stall after the first deltas: the message counts.
    let events = ask(
        &app,
        &user,
        &id,
        json!({"message": "[fake:stall-after-delta]"}),
    )
    .await;
    assert_eq!(names(&events), ["meta", "delta", "delta", "error"]);
    assert_eq!(used(&app, &user).await, 1);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_overall_deadline_ends_a_slow_but_steady_answer() {
    let timeouts = ChatTimeouts {
        overall: Duration::from_millis(500),
        first_delta: Duration::from_secs(5),
        gap: Duration::from_secs(5),
    };
    let app = start_with(Options {
        chat_timeouts: Some(timeouts),
        fake_delay: Duration::from_millis(150),
        ..Options::default()
    })
    .await;
    let user = app.user("student").await;
    let course = app.course(&user, "Deadline").await;
    let events = ask(
        &app,
        &user,
        &text(&course, "id"),
        json!({"message": "a slow answer"}),
    )
    .await;
    let kinds = names(&events);
    assert_eq!(kinds.first(), Some(&"meta"));
    assert_eq!(kinds.last(), Some(&"error"));
    assert!(kinds.contains(&"delta"));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_answer_is_saved_when_the_client_disconnects() {
    let app = start_with(Options {
        fake_delay: Duration::from_millis(30),
        ..Options::default()
    })
    .await;
    let user = app.user("student").await;
    let course = app.course(&user, "Disconnect").await;
    let id = text(&course, "id");

    // A real listener, so that a dropped client is a closed socket.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let mut response = reqwest::Client::new()
        .post(format!("http://{addr}/api/courses/{id}/chat"))
        .header("x-auth-token", &user.token)
        .json(&json!({"message": "leave early"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let first = response.chunk().await.unwrap().expect("a first chunk");
    assert!(String::from_utf8_lossy(&first).contains("event: meta"));
    drop(response);

    // The answer still gets saved.
    let mut saved = false;
    for _ in 0..100 {
        let listed = app
            .get(&format!("/api/courses/{id}/conversations"), &user)
            .await
            .json();
        if let Some(conv) = listed["conversations"].get(0) {
            let body = app
                .get(&format!("/api/conversations/{}", text(conv, "id")), &user)
                .await
                .json();
            if body["messages"].as_array().is_some_and(|m| m.len() == 2) {
                assert_eq!(body["messages"][1]["role"], "assistant");
                saved = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        saved,
        "the assistant message was not saved after the client left"
    );
    // The usage is recorded too.
    assert_eq!(used(&app, &user).await, 1);
    server.abort();
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn every_authenticated_route_refuses_a_missing_or_bad_token() {
    let app = start().await;
    let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let routes: Vec<(Method, String)> = vec![
        (Method::GET, "/api/me".into()),
        (Method::GET, "/api/models".into()),
        (Method::GET, "/api/courses".into()),
        (Method::POST, "/api/courses".into()),
        (Method::GET, format!("/api/courses/{id}")),
        (Method::PATCH, format!("/api/courses/{id}")),
        (Method::DELETE, format!("/api/courses/{id}")),
        (Method::POST, format!("/api/courses/{id}/invite-code")),
        (Method::POST, "/api/join".into()),
        (Method::GET, format!("/api/courses/{id}/members")),
        (Method::DELETE, format!("/api/courses/{id}/members/someone")),
        (Method::GET, format!("/api/courses/{id}/documents")),
        (Method::POST, format!("/api/courses/{id}/documents")),
        (
            Method::POST,
            format!("/api/courses/{id}/documents/{id}/complete"),
        ),
        (Method::DELETE, format!("/api/courses/{id}/documents/{id}")),
        (Method::GET, format!("/api/courses/{id}/conversations")),
        (Method::GET, format!("/api/conversations/{id}")),
        (Method::DELETE, format!("/api/conversations/{id}")),
        (Method::POST, format!("/api/courses/{id}/chat")),
        (Method::GET, format!("/api/courses/{id}/insights")),
    ];
    for (method, path) in &routes {
        for token in [None, Some("garbage"), Some("a.b.c")] {
            let reply = app
                .send(
                    method.clone(),
                    path,
                    token,
                    Some(json!({"name": "x", "message": "x", "inviteCode": "x"})),
                )
                .await;
            assert_eq!(
                reply.status,
                StatusCode::UNAUTHORIZED,
                "{method} {path} token={token:?}"
            );
            assert_eq!(reply.error_code(), "unauthenticated");
            assert_eq!(reply.content_type, "application/json");
        }
    }
    // Only the health route is open.
    assert_eq!(
        app.send(Method::GET, "/api/health", None, None)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_removed_student_cannot_chat_or_read() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");
    let events = ask(&app, &student, &id, json!({"message": "before removal"})).await;
    let conv = text(&events[0].1, "conversationId");

    let removed = app
        .delete(&format!("/api/courses/{id}/members/{}", student.id), &owner)
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);

    let reply = app
        .chat(&student, &id, json!({"message": "after removal"}))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.text);
    let reply = app
        .chat(
            &student,
            &id,
            json!({"conversationId": conv, "message": "again"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(
        app.get(&format!("/api/courses/{id}/documents"), &student)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        used(&app, &student).await,
        1,
        "no reservation for the refused chats"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_daily_quota_holds_under_parallel_requests() {
    let app = start_with(Options {
        user_daily_limit: 3,
        ..Options::default()
    })
    .await;
    let user = app.user("busy").await;
    let course = app.course(&user, "Parallel").await;
    let id = text(&course, "id");
    let replies =
        join_all((0..8).map(|n| app.chat(&user, &id, json!({"message": format!("parallel {n}")}))))
            .await;
    let ok = replies
        .iter()
        .filter(|r| r.status == StatusCode::OK)
        .count();
    let limited = replies
        .iter()
        .filter(|r| r.status == StatusCode::TOO_MANY_REQUESTS)
        .count();
    assert_eq!(
        (ok, limited),
        (3, 5),
        "{:?}",
        replies.iter().map(|r| r.status).collect::<Vec<_>>()
    );
    assert_eq!(used(&app, &user).await, 3);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_global_cap_holds_under_parallel_requests() {
    let app = start_with(Options {
        global_daily_limit: 2,
        ..Options::default()
    })
    .await;
    let owner = app.user("owner").await;
    let course = app.course(&owner, "Global").await;
    let id = text(&course, "id");
    let mut students = Vec::new();
    for n in 0..5 {
        let student = app.user(&format!("s{n}")).await;
        app.post(
            "/api/join",
            &student,
            json!({"inviteCode": course["inviteCode"]}),
        )
        .await;
        students.push(student);
    }
    let replies = join_all(
        students
            .iter()
            .map(|s| app.chat(s, &id, json!({"message": "hi"}))),
    )
    .await;
    let ok = replies
        .iter()
        .filter(|r| r.status == StatusCode::OK)
        .count();
    let capped = replies
        .iter()
        .filter(|r| r.status == StatusCode::SERVICE_UNAVAILABLE)
        .count();
    assert_eq!((ok, capped), (2, 3));
    // Every refused user got the reservation back.
    let mut total = 0;
    for s in &students {
        total += used(&app, s).await;
    }
    assert_eq!(total, 2);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_global_token_cap_returns_503_before_the_model_call() {
    let app = start_with(Options {
        global_tokens: 50,
        ..Options::default()
    })
    .await;
    let user = app.user("student").await;
    let course = app.course(&user, "Tokens").await;
    let id = text(&course, "id");
    // The first answer uses more than 50 tokens.
    let events = ask(&app, &user, &id, json!({"message": "first"})).await;
    assert_eq!(events.last().unwrap().0, "done");
    let reply = app.chat(&user, &id, json!({"message": "second"})).await;
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.text
    );
    assert_eq!(reply.error_code(), "capacity_exhausted");
    assert_eq!(
        used(&app, &user).await,
        1,
        "nothing was reserved for the refused message"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn course_and_document_limits_hold_under_parallel_creates() {
    let app = start_with(Options {
        courses_owned: 2,
        documents_per_course: 2,
        ..Options::default()
    })
    .await;
    let owner = app.user("teacher").await;
    let replies = join_all((0..6).map(|n| {
        app.post(
            "/api/courses",
            &owner,
            json!({"name": format!("Course {n}")}),
        )
    }))
    .await;
    let created: Vec<_> = replies
        .iter()
        .filter(|r| r.status == StatusCode::CREATED)
        .collect();
    let refused = replies
        .iter()
        .filter(|r| r.status == StatusCode::CONFLICT && r.error_code() == "limit_reached")
        .count();
    assert_eq!(
        (created.len(), refused),
        (2, 4),
        "{:?}",
        replies.iter().map(|r| r.status).collect::<Vec<_>>()
    );
    let listed = app.get("/api/courses", &owner).await.json();
    assert_eq!(listed["courses"].as_array().unwrap().len(), 2);

    let id = text(&created[0].json()["course"], "id");
    let body = json!({"filename": "a.pdf", "sizeBytes": 10, "contentType": "application/pdf"});
    let path = format!("/api/courses/{id}/documents");
    let replies = join_all((0..6).map(|_| app.post(&path, &owner, body.clone()))).await;
    let created = replies
        .iter()
        .filter(|r| r.status == StatusCode::CREATED)
        .count();
    let refused = replies
        .iter()
        .filter(|r| r.status == StatusCode::CONFLICT)
        .count();
    assert_eq!((created, refused), (2, 4));
    let docs = app.get(&path, &owner).await.json();
    assert_eq!(docs["documents"].as_array().unwrap().len(), 2);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn deleting_a_course_frees_the_owner_slot_and_a_document_frees_its_slot() {
    let app = start_with(Options {
        courses_owned: 1,
        documents_per_course: 1,
        ..Options::default()
    })
    .await;
    let owner = app.user("teacher").await;
    let first = app.course(&owner, "First").await;
    assert_eq!(
        app.post("/api/courses", &owner, json!({"name": "Second"}))
            .await
            .status,
        StatusCode::CONFLICT
    );

    let id = text(&first, "id");
    let path = format!("/api/courses/{id}/documents");
    let body = json!({"filename": "a.pdf", "sizeBytes": 10, "contentType": "application/pdf"});
    let doc = app.post(&path, &owner, body.clone()).await;
    assert_eq!(doc.status, StatusCode::CREATED);
    assert_eq!(
        app.post(&path, &owner, body.clone()).await.status,
        StatusCode::CONFLICT
    );
    let doc_id = text(&doc.json()["document"], "id");
    assert_eq!(
        app.delete(&format!("{path}/{doc_id}"), &owner).await.status,
        StatusCode::ACCEPTED
    );
    // The slot comes back when the worker deletes the record. Here the test plays the worker.
    assert_eq!(
        app.post(&path, &owner, body.clone()).await.status,
        StatusCode::CONFLICT
    );
    app.state.store.delete_document(&id, &doc_id).await.unwrap();
    assert_eq!(
        app.post(&path, &owner, body).await.status,
        StatusCode::CREATED
    );

    assert_eq!(
        app.delete(&format!("/api/courses/{id}"), &owner)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    // The slot comes back when the worker finishes the delete. Here the test plays the worker.
    assert_eq!(
        app.post("/api/courses", &owner, json!({"name": "Second"}))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let record = app.state.store.get_course(&id).await.unwrap().unwrap();
    app.state.store.delete_course_data(&record).await.unwrap();
    assert_eq!(
        app.post("/api/courses", &owner, json!({"name": "Second"}))
            .await
            .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_delete_that_stays_deleting_for_twenty_minutes_sends_the_message_again() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "Stuck").await;
    let id = text(&course, "id");
    let reply = app
        .post(
            &format!("/api/courses/{id}/documents"),
            &owner,
            json!({"filename": "a.pdf", "sizeBytes": 3, "contentType": "application/pdf"}),
        )
        .await;
    let doc_id = text(&reply.json()["document"], "id");
    let path = format!("/api/courses/{id}/documents/{doc_id}");
    assert_eq!(app.delete(&path, &owner).await.status, StatusCode::ACCEPTED);
    assert_eq!(app.queued_messages().await.len(), 1);
    // A repeat right away sends nothing.
    assert_eq!(app.delete(&path, &owner).await.status, StatusCode::ACCEPTED);
    assert!(app.queued_messages().await.is_empty());

    // The record says that the delete started 30 minutes ago.
    let long_ago =
        openbook_core::domain::timestamp(chrono::Utc::now() - chrono::Duration::minutes(30));
    app.dynamodb
        .update_item()
        .table_name(&app.table)
        .key("pk", AttributeValue::S(format!("COURSE#{id}")))
        .key("sk", AttributeValue::S(format!("DOC#{doc_id}")))
        .update_expression("SET updatedAt = :at")
        .expression_attribute_values(":at", AttributeValue::S(long_ago))
        .send()
        .await
        .unwrap();
    let reply = app.delete(&path, &owner).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert_eq!(reply.json()["document"]["status"], "deleting");
    assert_eq!(
        app.queued_messages().await,
        vec![json!({"action": "delete", "courseId": id, "documentId": doc_id})]
    );
    // The message time moved, so the next repeat sends nothing again.
    assert_eq!(app.delete(&path, &owner).await.status, StatusCode::ACCEPTED);
    assert!(app.queued_messages().await.is_empty());
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_course_delete_hides_the_course_at_once_and_the_owner_can_repeat_a_failed_one() {
    let app = start().await;
    let owner = app.user("teacher").await;
    let student = app.user("student").await;
    let late = app.user("late").await;
    let course = app.course_with_student(&owner, &student).await;
    let id = text(&course, "id");
    let code = text(&course, "inviteCode");
    ask(&app, &student, &id, json!({"message": "a question"})).await;

    // The queue is broken, so the delete cannot finish.
    let broken = app.router_with_queue("https://127.0.0.1:9/queue-that-is-gone.fifo");
    let path = format!("/api/courses/{id}");
    let reply = support::send_to(&broken, Method::DELETE, &path, Some(&owner.token), None).await;
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.text
    );
    assert_eq!(reply.error_code(), "unavailable");

    // Even so, the course is gone for everyone, and the invite is dead.
    for user in [&owner, &student] {
        assert_eq!(app.get(&path, user).await.status, StatusCode::NOT_FOUND);
        assert_eq!(
            app.get("/api/courses", user).await.json()["courses"],
            json!([])
        );
        assert_eq!(
            app.chat(user, &id, json!({"message": "hi"})).await.status,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        app.post("/api/join", &late, json!({"inviteCode": code}))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.delete(&path, &late).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        app.delete(&path, &student).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(app.queued_messages().await.is_empty());
    // The items wait for the worker.
    let meta = item(&app, &format!("COURSE#{id}"), "META")
        .await
        .expect("the course item");
    assert_eq!(
        meta.get("status"),
        Some(&AttributeValue::S("deleting".into()))
    );

    // The owner repeats the request, and this time the message is sent. A third call sends it again.
    let reply = app.delete(&path, &owner).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text);
    assert_eq!(
        app.queued_messages().await,
        vec![json!({"action": "delete_course", "courseId": id})]
    );
    assert_eq!(
        app.delete(&path, &owner).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.queued_messages().await.len(), 1);

    // The owner slot comes back when the worker finishes. Here the test plays the worker.
    let record = app
        .state
        .store
        .get_course(&id)
        .await
        .unwrap()
        .expect("still stored");
    assert!(record.deleting);
    app.state.store.delete_course_data(&record).await.unwrap();
    assert!(item(&app, &format!("COURSE#{id}"), "META").await.is_none());
    // Only the questions stay, and every one has a TTL.
    let left = app
        .dynamodb
        .query()
        .table_name(&app.table)
        .key_condition_expression("pk = :pk")
        .expression_attribute_values(":pk", AttributeValue::S(format!("COURSE#{id}")))
        .consistent_read(true)
        .send()
        .await
        .unwrap();
    assert_eq!(left.items().len(), 1);
    for item in left.items() {
        assert!(matches!(item.get("sk"), Some(AttributeValue::S(sk)) if sk.starts_with("Q#")));
        assert!(item.contains_key("ttl"));
    }
    assert_eq!(
        app.delete(&path, &owner).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn failed_joins_are_limited_per_user_per_day() {
    let app = start_with(Options {
        failed_joins_per_day: 3,
        ..Options::default()
    })
    .await;
    let owner = app.user("teacher").await;
    let guesser = app.user("guesser").await;
    let other = app.user("other").await;
    let course = app.course(&owner, "Guessing").await;
    for guess in ["AAAAAAAA", "BBBBBBBB", "not a code"] {
        let reply = app
            .post("/api/join", &guesser, json!({"inviteCode": guess}))
            .await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{guess}");
    }
    // The limit is reached. Even the right code is refused now.
    let reply = app
        .post(
            "/api/join",
            &guesser,
            json!({"inviteCode": course["inviteCode"]}),
        )
        .await;
    assert_eq!(
        reply.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        reply.text
    );
    assert_eq!(reply.error_code(), "quota_exceeded");
    // Other users are not affected, and a good join does not count against anyone.
    let reply = app
        .post(
            "/api/join",
            &other,
            json!({"inviteCode": course["inviteCode"]}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn only_the_last_twenty_messages_go_to_the_model() {
    let app = start().await;
    let user = app.user("talker").await;
    let course = app.course(&user, "History").await;
    let id = text(&course, "id");

    let mut conv: Option<String> = None;
    let mut input_tokens = Vec::new();
    for n in 1..=16 {
        // The same length each time, so the prompt grows only with the history.
        let mut body = json!({"message": format!("message {n:02}")});
        if let Some(c) = &conv {
            body["conversationId"] = json!(c);
        }
        let events = ask(&app, &user, &id, body).await;
        conv.get_or_insert_with(|| text(&events[0].1, "conversationId"));
        input_tokens.push(
            events.last().unwrap().1["usage"]["inputTokens"]
                .as_u64()
                .unwrap(),
        );
    }
    // The first turns grow. From the 11th message on, 20 earlier messages exist, and the size stays flat.
    assert!(
        input_tokens[0] < input_tokens[4] && input_tokens[4] < input_tokens[9],
        "{input_tokens:?}"
    );
    assert!(
        input_tokens[10..].windows(2).all(|w| w[0] == w[1]),
        "{input_tokens:?}"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn bad_path_encodings_get_the_json_404() {
    let app = start().await;
    let user = app.user("alice").await;
    for path in [
        "/api/courses/%FF",
        "/api/conversations/%FF",
        "/api/courses/%FF/documents/%FE/complete",
    ] {
        let reply = app.send(Method::GET, path, Some(&user.token), None).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(reply.error_code(), "not_found");
        assert_eq!(reply.content_type, "application/json");
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn every_response_has_a_request_id() {
    let app = start().await;
    let first = reqwest_free_request_id(&app, "/api/health").await;
    let second = reqwest_free_request_id(&app, "/api/health").await;
    assert_eq!(first.len(), 26);
    assert_ne!(first, second);
}

async fn reqwest_free_request_id(app: &TestApp, path: &str) -> String {
    let request = axum::http::Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(app.router.clone(), request)
        .await
        .unwrap();
    response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn messages_saved_after_a_conversation_was_deleted_expire() {
    let app = start().await;
    let user = app.user("alice").await;
    let course = app.course(&user, "Orphans").await;
    let events = ask(
        &app,
        &user,
        &text(&course, "id"),
        json!({"message": "to be deleted"}),
    )
    .await;
    let conv = text(&events[0].1, "conversationId");
    assert_eq!(
        app.delete(&format!("/api/conversations/{conv}"), &user)
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    // A chat that was still running writes an answer after the delete.
    let store = &app.state.store;
    store
        .add_message(
            &conv,
            &openbook_core::store::NewMessage {
                id: openbook_core::ids::new_id(),
                role: openbook_core::domain::MessageRole::Assistant,
                content: "late".into(),
                created_at: openbook_core::domain::now(),
                sources: None,
                trace: None,
                model: Some("fake-a".into()),
                style: None,
                usage: Default::default(),
            },
        )
        .await
        .unwrap();
    assert!(
        !store
            .touch_conversation(
                &user.id,
                &conv,
                "fake-a",
                openbook_core::domain::TutoringStyle::Socratic,
                &openbook_core::domain::now(),
            )
            .await
            .unwrap()
    );
    store.expire_conversation_messages(&conv).await.unwrap();
    let left = app
        .dynamodb
        .query()
        .table_name(&app.table)
        .key_condition_expression("pk = :pk")
        .expression_attribute_values(":pk", AttributeValue::S(format!("CONV#{conv}")))
        .consistent_read(true)
        .send()
        .await
        .unwrap();
    assert_eq!(left.items().len(), 1);
    assert!(left.items()[0].contains_key("ttl"));
}
