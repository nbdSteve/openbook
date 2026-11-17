//! The model catalog: the list of models that courses and chats may use.
//!
//! It comes from `OPENBOOK_MODELS` (a JSON array). The API validates it at start-up.

use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_OUTPUT_TOKENS: i32 = 2000;
const MAX_OUTPUT_TOKENS_LIMIT: i32 = 64_000;

/// Vendors that model IDs may name. The CDK app uses the same list.
pub const ALLOWED_VENDORS: [&str; 8] = [
    "anthropic",
    "openai",
    "amazon",
    "meta",
    "mistral",
    "cohere",
    "deepseek",
    "qwen",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Bedrock,
    /// Deterministic local provider. The API refuses it in Lambda.
    Fake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    /// Processed in Australia.
    Au,
    /// The provider may process data outside Australia.
    Global,
}

/// One catalog entry, as written in `OPENBOOK_MODELS`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    pub vendor: String,
    pub provider: ProviderKind,
    pub model_id: String,
    pub residency: Residency,
    #[serde(default)]
    pub max_output_tokens: Option<i32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// True when the model accepts images (page transcription needs it).
    #[serde(default)]
    pub vision: bool,
    /// List price in US dollars per million input tokens. The demo budget and later billing use it.
    #[serde(default)]
    pub input_usd_per_million: Option<f64>,
    /// List price in US dollars per million output tokens.
    #[serde(default)]
    pub output_usd_per_million: Option<f64>,
}

/// The largest price per million tokens that the catalog accepts. It catches a typo such as 300000.
const MAX_USD_PER_MILLION: f64 = 1000.0;

impl ModelEntry {
    /// The output token limit (server-fixed).
    pub fn max_tokens(&self) -> i32 {
        self.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
    }

    /// The cost of a call in millionths of a US dollar, rounded up. `None` without prices.
    /// One token at 1 US dollar per million tokens costs exactly 1 millionth of a dollar, so the
    /// price per million is the cost per token in these units.
    pub fn cost_micro_usd(&self, input_tokens: u64, output_tokens: u64) -> Option<u64> {
        let (input, output) = (self.input_usd_per_million?, self.output_usd_per_million?);
        let cost = input * input_tokens as f64 + output * output_tokens as f64;
        Some(cost.ceil() as u64)
    }
}

/// How a model is shown to clients (`GET /api/models`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub vendor: String,
    pub residency: Residency,
    pub is_default: bool,
}

/// The validated catalog.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<ModelEntry>,
    default_chat: String,
    fast: String,
}

/// The residency that a Bedrock model ID implies. This is an allowlist of three shapes:
/// `au.<vendor>.<model>` is `au`, `<vendor>.<model>` is `au` (on demand in Sydney), and
/// `global.<vendor>.<model>` is `global`. Anything else is refused.
pub fn residency_of(model_id: &str) -> Result<Residency, String> {
    let refused = || {
        format!(
            "modelId {model_id:?} must be au.<vendor>.<model>, <vendor>.<model>, or global.<vendor>.<model> \
             with a vendor from {ALLOWED_VENDORS:?}"
        )
    };
    let (residency, rest) = match model_id.split_once('.') {
        Some(("au", rest)) => (Residency::Au, rest),
        Some(("global", rest)) => (Residency::Global, rest),
        _ => (Residency::Au, model_id),
    };
    let Some((vendor, model)) = rest.split_once('.') else {
        return Err(refused());
    };
    let model_ok = !model.is_empty()
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'));
    if ALLOWED_VENDORS.contains(&vendor) && model_ok {
        Ok(residency)
    } else {
        Err(refused())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Catalog {
    /// Parses and validates the catalog. Returns every problem found.
    pub fn parse(json: &str, default_chat: &str, fast: &str) -> Result<Catalog, Vec<String>> {
        let entries: Vec<ModelEntry> = serde_json::from_str(json)
            .map_err(|e| vec![format!("OPENBOOK_MODELS is not a valid model list: {e}")])?;
        let mut problems = Vec::new();
        if entries.is_empty() {
            problems.push("OPENBOOK_MODELS must have at least one model".into());
        }
        let mut seen = std::collections::HashSet::new();
        for entry in &entries {
            let id = &entry.id;
            if !valid_id(id) {
                problems.push(format!("model id {id:?} must match [a-z0-9-]{{1,40}}"));
            }
            if !seen.insert(id.as_str()) {
                problems.push(format!("model id {id:?} is used twice"));
            }
            if entry.label.trim().is_empty()
                || entry.vendor.trim().is_empty()
                || entry.model_id.trim().is_empty()
            {
                problems.push(format!(
                    "model {id:?} needs a label, a vendor, and a modelId"
                ));
            }
            // Fake entries (local only) skip the shape rule and keep the residency they declare.
            if entry.provider == ProviderKind::Bedrock {
                match residency_of(&entry.model_id) {
                    Ok(implied) if implied != entry.residency => problems.push(format!(
                        "model {id:?} says residency {:?} but its modelId implies {implied:?}",
                        entry.residency
                    )),
                    Ok(_) => {}
                    Err(reason) => problems.push(format!("model {id:?}: {reason}")),
                }
            }
            if let Some(max) = entry.max_output_tokens
                && !(1..=MAX_OUTPUT_TOKENS_LIMIT).contains(&max)
            {
                problems.push(format!(
                    "model {id:?}: maxOutputTokens must be 1..={MAX_OUTPUT_TOKENS_LIMIT}"
                ));
            }
            if let Some(t) = entry.temperature
                && !(0.0..=2.0).contains(&t)
            {
                problems.push(format!("model {id:?}: temperature must be 0..=2"));
            }
            if entry.input_usd_per_million.is_some() != entry.output_usd_per_million.is_some() {
                problems.push(format!(
                    "model {id:?}: set both inputUsdPerMillion and outputUsdPerMillion, or neither"
                ));
            }
            for (name, price) in [
                ("inputUsdPerMillion", entry.input_usd_per_million),
                ("outputUsdPerMillion", entry.output_usd_per_million),
            ] {
                if let Some(p) = price
                    && !(p.is_finite() && (0.0..=MAX_USD_PER_MILLION).contains(&p))
                {
                    problems.push(format!(
                        "model {id:?}: {name} must be 0..={MAX_USD_PER_MILLION}"
                    ));
                }
            }
        }
        for (name, id) in [
            ("OPENBOOK_DEFAULT_CHAT_MODEL", default_chat),
            ("OPENBOOK_FAST_MODEL", fast),
        ] {
            match entries.iter().find(|e| e.id == id) {
                None => problems.push(format!("{name} {id:?} is not in the catalog")),
                Some(e) if e.residency != Residency::Au => {
                    problems.push(format!("{name} {id:?} must have residency au"))
                }
                Some(_) => {}
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Catalog {
            entries,
            default_chat: default_chat.to_owned(),
            fast: fast.to_owned(),
        })
    }

    pub fn entries(&self) -> &[ModelEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn default_chat_id(&self) -> &str {
        &self.default_chat
    }

    pub fn fast_id(&self) -> &str {
        &self.fast
    }

    /// The ID to use for a stored model ID: itself when it is in the catalog, else the default.
    pub fn effective_id<'a>(&'a self, stored: &'a str) -> &'a str {
        if self.get(stored).is_some() {
            stored
        } else {
            &self.default_chat
        }
    }

    /// Checks the model for page transcription: it must be in the catalog, accept images, and run in Australia.
    pub fn check_transcribe_model(&self, id: &str) -> Result<(), String> {
        match self.get(id) {
            None => Err(format!(
                "OPENBOOK_TRANSCRIBE_MODEL {id:?} is not in the catalog"
            )),
            Some(e) if !e.vision => Err(format!(
                "OPENBOOK_TRANSCRIBE_MODEL {id:?} must have vision set to true"
            )),
            Some(e) if e.residency != Residency::Au => Err(format!(
                "OPENBOOK_TRANSCRIBE_MODEL {id:?} must have residency au"
            )),
            Some(_) => Ok(()),
        }
    }

    pub fn has_fake(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.provider == ProviderKind::Fake)
    }

    pub fn infos(&self) -> Vec<ModelInfo> {
        self.entries
            .iter()
            .map(|e| ModelInfo {
                id: e.id.clone(),
                label: e.label.clone(),
                vendor: e.vendor.clone(),
                residency: e.residency,
                is_default: e.id == self.default_chat,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(id: &str, model_id: &str, residency: &str) -> serde_json::Value {
        json!({"id": id, "label": id, "vendor": "v", "provider": "bedrock", "modelId": model_id, "residency": residency})
    }

    fn catalog(entries: serde_json::Value) -> Result<Catalog, Vec<String>> {
        Catalog::parse(&entries.to_string(), "a", "a")
    }

    fn problems(entries: serde_json::Value) -> String {
        catalog(entries).unwrap_err().join(" | ")
    }

    #[test]
    fn parses_a_good_catalog() {
        let c = Catalog::parse(
            &json!([
                entry("a", "au.anthropic.claude-x", "au"),
                entry("b", "openai.gpt-oss-120b-1:0", "au"),
                entry("c", "global.openai.gpt-6.1-sol", "global"),
                {"id": "d", "label": "D", "vendor": "v", "provider": "fake", "modelId": "fake", "residency": "au",
                 "maxOutputTokens": 500, "temperature": 0.2}
            ])
            .to_string(),
            "a",
            "b",
        )
        .unwrap();
        assert_eq!(c.entries().len(), 4);
        assert_eq!(c.default_chat_id(), "a");
        assert_eq!(c.fast_id(), "b");
        assert_eq!(c.get("a").unwrap().max_tokens(), 2000);
        assert_eq!(c.get("d").unwrap().max_tokens(), 500);
        assert_eq!(c.get("d").unwrap().temperature, Some(0.2));
        assert!(c.has_fake());
        let infos = c.infos();
        assert_eq!(
            infos
                .iter()
                .filter(|i| i.is_default)
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(
            serde_json::to_value(&infos[2]).unwrap()["residency"],
            "global"
        );
        assert_eq!(serde_json::to_value(&infos[0]).unwrap()["isDefault"], true);
    }

    #[test]
    fn effective_id_falls_back_to_the_default() {
        let c = catalog(json!([
            entry("a", "au.anthropic.y", "au"),
            entry("b", "au.anthropic.z", "au")
        ]))
        .unwrap();
        assert_eq!(c.effective_id("b"), "b");
        assert_eq!(c.effective_id("gone"), "a");
    }

    #[test]
    fn rejects_bad_json_and_empty_lists() {
        assert!(Catalog::parse("nope", "a", "a").is_err());
        assert!(Catalog::parse("{}", "a", "a").is_err());
        assert!(problems(json!([])).contains("at least one"));
    }

    #[test]
    fn rejects_bad_ids() {
        for bad in ["", "Upper", "has space", "under_score", &"x".repeat(41)] {
            assert!(
                problems(json!([entry(bad, "au.anthropic.y", "au")])).contains("must match"),
                "{bad:?}"
            );
        }
        assert!(
            problems(json!([
                entry("a", "au.anthropic.y", "au"),
                entry("a", "au.anthropic.z", "au")
            ]))
            .contains("used twice")
        );
    }

    #[test]
    fn residency_must_match_the_model_id() {
        assert!(problems(json!([entry("a", "au.anthropic.y", "global")])).contains("implies"));
        assert!(problems(json!([entry("a", "global.anthropic.y", "au")])).contains("implies"));
        assert!(problems(json!([entry("a", "openai.gpt-x", "global")])).contains("implies"));
    }

    #[test]
    fn other_profile_prefixes_are_refused() {
        for prefix in ["us", "eu", "apac", "jp", "us-gov"] {
            let p = problems(json!([entry(
                "a",
                &format!("{prefix}.anthropic.claude-x"),
                "au"
            )]));
            assert!(p.contains("must be au."), "{prefix}: {p}");
        }
        assert!(
            problems(json!([entry(
                "a",
                "arn:aws:bedrock:ap-southeast-2::foundation-model/x",
                "au"
            )]))
            .contains("must be au.")
        );
    }

    #[test]
    fn default_and_fast_must_exist_and_be_au() {
        let list = json!([
            entry("a", "au.anthropic.y", "au"),
            entry("g", "global.anthropic.y", "global")
        ]);
        let text = list.to_string();
        assert!(
            Catalog::parse(&text, "missing", "a")
                .unwrap_err()
                .join(" ")
                .contains("not in the catalog")
        );
        assert!(Catalog::parse(&text, "a", "missing").is_err());
        assert!(
            Catalog::parse(&text, "g", "a")
                .unwrap_err()
                .join(" ")
                .contains("must have residency au")
        );
        assert!(Catalog::parse(&text, "a", "g").is_err());
    }

    #[test]
    fn rejects_bad_limits_and_unknown_fields() {
        let mut e = entry("a", "au.anthropic.y", "au");
        e["maxOutputTokens"] = json!(0);
        assert!(problems(json!([e])).contains("maxOutputTokens"));
        let mut e = entry("a", "au.anthropic.y", "au");
        e["temperature"] = json!(5);
        assert!(problems(json!([e])).contains("temperature"));
        let mut e = entry("a", "au.anthropic.y", "au");
        e["surprise"] = json!(true);
        assert!(catalog(json!([e])).is_err());
        let mut e = entry("a", "au.anthropic.y", "au");
        e["provider"] = json!("openai");
        assert!(catalog(json!([e])).is_err());
    }

    #[test]
    fn the_transcribe_model_must_have_vision_and_residency_au() {
        let list = json!([
            {"id": "seeing", "label": "S", "vendor": "v", "provider": "fake", "modelId": "x", "residency": "au", "vision": true},
            {"id": "blind", "label": "B", "vendor": "v", "provider": "fake", "modelId": "x", "residency": "au"},
            {"id": "far", "label": "F", "vendor": "v", "provider": "fake", "modelId": "x", "residency": "global", "vision": true}
        ]);
        let c = Catalog::parse(&list.to_string(), "seeing", "seeing").unwrap();
        assert!(c.check_transcribe_model("seeing").is_ok());
        assert!(
            c.check_transcribe_model("blind")
                .unwrap_err()
                .contains("vision")
        );
        assert!(
            c.check_transcribe_model("far")
                .unwrap_err()
                .contains("residency au")
        );
        assert!(
            c.check_transcribe_model("gone")
                .unwrap_err()
                .contains("not in the catalog")
        );
        assert!(!c.get("blind").unwrap().vision);
    }

    #[test]
    fn only_the_three_shapes_with_allowed_vendors_pass() {
        for ok in [
            "au.anthropic.claude-sonnet-4-6",
            "anthropic.claude-3-haiku-20240307-v1:0",
            "openai.gpt-oss-120b-1:0",
            "global.openai.gpt-6.1-sol",
            "au.amazon.nova-pro-v1:0",
        ] {
            assert!(residency_of(ok).is_ok(), "{ok}");
        }
        for bad in [
            "claude",
            "au.claude",
            "anthropic",
            "anthropic.",
            "au.anthropic.",
            "global.nobody.model",
            "nobody.model",
            "xx.anthropic.claude",
            "us.anthropic.claude",
            "global.au.anthropic.claude",
            "apac.anthropic.claude-x",
            "au.anthropic.bad model",
            "au.anthropic.model/../x",
            "",
        ] {
            assert!(residency_of(bad).is_err(), "{bad}");
        }
        assert_eq!(residency_of("au.anthropic.x").unwrap(), Residency::Au);
        assert_eq!(
            residency_of("meta.llama3-1-70b-instruct-v1:0").unwrap(),
            Residency::Au
        );
        assert_eq!(
            residency_of("global.anthropic.x").unwrap(),
            Residency::Global
        );
    }

    #[test]
    fn fake_entries_skip_the_shape_rule_and_keep_their_residency() {
        let fake = |id: &str, model_id: &str, residency: &str| json!({"id": id, "label": id, "vendor": "v", "provider": "fake", "modelId": model_id, "residency": residency});
        let c = Catalog::parse(
            &json!([
                fake("a", "whatever", "au"),
                fake("b", "au.nobody", "global")
            ])
            .to_string(),
            "a",
            "a",
        )
        .unwrap();
        assert_eq!(c.get("b").unwrap().residency, Residency::Global);
    }
}
