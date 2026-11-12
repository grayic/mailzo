//! AI with the person's own key, from the service they choose. Calls go straight from
//! this device to that company, and the key travels between their devices only inside
//! the vault.

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::search::{parse_plain, SearchFilters};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub(crate) const AI_KEY: &str = "ai-key";
/// The longest answer any request may get back.
pub(crate) const MOST_ANSWER_TOKENS: i64 = 4000;
/// Passed along where a key would go. A plan signs in by its own program instead.
const PLAN_NOT_A_KEY: &str = "plan";
pub(crate) const AI_URL: &str = "https://api.anthropic.com";
/// The models worked out for the chosen service, and the one picked for writing if any.
pub(crate) const AI_PICKS: &str = "ai_picks";
const MOST_MODELS_LISTED: usize = 100;

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Anthropic,
    /// The shape OpenAI made popular, which most other services also accept.
    OpenAi,
}

/// A company whose models can be used with a key from them. Model names change often,
/// so each service is asked what it has, and these words pick from the answer.
struct Service {
    id: &'static str,
    name: &'static str,
    url: &'static str,
    style: Style,
    keys_page: &'static str,
    /// For writing and answering when the person leaves it on Automatic: good, at a fair price.
    balanced: &'static [&'static str],
    /// In order of preference, for the quick, inexpensive model used unless another is chosen.
    fast: &'static [&'static str],
}

const SERVICES: &[Service] = &[
    Service {
        id: "anthropic",
        name: "Claude",
        url: AI_URL,
        style: Style::Anthropic,
        keys_page: "https://console.anthropic.com/settings/keys",
        fast: &["haiku"],
        balanced: &["sonnet"],
    },
    Service {
        id: "openai",
        name: "ChatGPT",
        url: "https://api.openai.com/v1",
        style: Style::OpenAi,
        keys_page: "https://platform.openai.com/api-keys",
        fast: &["luna", "nano", "mini"],
        balanced: &["mini", "gpt"],
    },
    Service {
        id: "gemini",
        name: "Gemini",
        url: "https://generativelanguage.googleapis.com/v1beta/openai",
        style: Style::OpenAi,
        keys_page: "https://aistudio.google.com/apikey",
        fast: &["flash"],
        balanced: &["flash"],
    },
    Service {
        id: "xai",
        name: "Grok",
        url: "https://api.x.ai/v1",
        style: Style::OpenAi,
        keys_page: "https://console.x.ai",
        fast: &["fast", "mini", "grok"],
        balanced: &["grok"],
    },
    Service {
        id: "deepseek",
        name: "DeepSeek",
        url: "https://api.deepseek.com",
        style: Style::OpenAi,
        keys_page: "https://platform.deepseek.com/api_keys",
        fast: &["flash", "chat"],
        balanced: &["chat"],
    },
    Service {
        id: "mistral",
        name: "Mistral",
        url: "https://api.mistral.ai/v1",
        style: Style::OpenAi,
        keys_page: "https://console.mistral.ai/api-keys",
        fast: &["small", "medium"],
        balanced: &["medium", "small"],
    },
    Service {
        id: "groq",
        name: "Groq",
        url: "https://api.groq.com/openai/v1",
        style: Style::OpenAi,
        keys_page: "https://console.groq.com/keys",
        fast: &["instant", "8b", "llama"],
        balanced: &["70b", "versatile"],
    },
    Service {
        id: "openrouter",
        name: "OpenRouter",
        url: "https://openrouter.ai/api/v1",
        style: Style::OpenAi,
        keys_page: "https://openrouter.ai/keys",
        fast: &["openrouter/auto"],
        balanced: &["openrouter/auto"],
    },
];

/// Every service's id, for the settings to check against.
pub(crate) const AI_SERVICE_IDS: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "xai",
    "deepseek",
    "mistral",
    "groq",
    "openrouter",
];

/// What was worked out from a service's list of models.
#[derive(Default, Serialize, Deserialize)]
struct Picks {
    fast: String,
    /// Chosen by the person for writing and answering. Empty leaves it to the app.
    chosen: String,
    models: Vec<String>,
}

/// Models that do something other than write text.
const NOT_FOR_WRITING: &[&str] = &[
    "embed",
    "whisper",
    "tts",
    "audio",
    "image",
    "realtime",
    "moderation",
    "transcri",
    "dall",
    "sora",
    "guard",
    "ocr",
    "search",
    "research",
    "codex",
    "instruct",
    "computer",
    "rerank",
    "aqa",
    "veo",
    "imagen",
    "live",
    "robotics",
];

/// The numbers in a model's name, newest first when sorted. Long runs are dates, not versions.
fn version_of(id: &str) -> Vec<u32> {
    id.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty() && run.len() <= 2)
        .filter_map(|run| run.parse().ok())
        .take(2)
        .collect()
}

fn usable_models(ids: Vec<String>) -> Vec<String> {
    let mut models: Vec<String> = ids
        .into_iter()
        .map(|id| id.trim_start_matches("models/").to_string())
        .filter(|id| {
            let lower = id.to_lowercase();
            !NOT_FOR_WRITING.iter().any(|word| lower.contains(word))
        })
        .collect();
    models.sort_by(|a, b| {
        version_of(b)
            .cmp(&version_of(a))
            .then(a.len().cmp(&b.len()))
            .then(a.cmp(b))
    });
    models.dedup();
    models.truncate(MOST_MODELS_LISTED);
    models
}

/// The newest model matching the earliest word that matches at all. Previews come last.
fn pick_model(models: &[String], words: &[&str]) -> Option<String> {
    let settled = |id: &&String| !id.contains("preview") && !id.contains("-exp");
    for word in words {
        let matching = |id: &&String| {
            let lower = id.to_lowercase();
            lower.contains(word) && (*word != "flash" || !lower.contains("lite"))
        };
        let found = models
            .iter()
            .filter(matching)
            .find(settled)
            .or_else(|| models.iter().find(matching));
        if let Some(found) = found {
            return Some(found.clone());
        }
    }
    models.iter().find(settled).or(models.first()).cloned()
}

/// Some services wrap the answer in a code block, or say a sentence before or after it, even
/// when asked not to. This finds the JSON object inside.
pub(crate) fn without_code_fence(text: &str) -> &str {
    let text = text.trim();
    if text.starts_with('{') && text.ends_with('}') {
        return text;
    }
    // A code block anywhere in the answer.
    if let Some(start) = text.find("```") {
        let inner = &text[start + 3..];
        let inner = inner.strip_prefix("json").unwrap_or(inner);
        let inner = inner.find("```").map_or(inner, |end| &inner[..end]).trim();
        if inner.starts_with('{') {
            return inner;
        }
    }
    // Otherwise from the first { to the last }.
    match (text.find('{'), text.rfind('}')) {
        (Some(open), Some(close)) if open < close => &text[open..=close],
        _ => text,
    }
}

fn failure(status: u16, said: &str) -> CoreError {
    let said = said.to_lowercase();
    match status {
        401 | 403 => ai_error("ai_key", "That key wasn't accepted."),
        402 => ai_error("ai_credit", "Your AI account is out of credit."),
        429 if said.contains("quota") || said.contains("credit") || said.contains("balance") => {
            ai_error("ai_credit", "Your AI account is out of credit.")
        }
        429 => ai_error("ai_busy", "Your AI account has reached its limit for now."),
        404 => ai_error("ai_model", "That AI model isn't available any more."),
        400 if said.contains("api key") || said.contains("api_key") => {
            ai_error("ai_key", "That key wasn't accepted.")
        }
        400 if said.contains("credit") || said.contains("balance") => {
            ai_error("ai_credit", "Your AI account is out of credit.")
        }
        400 if said.contains("model")
            && (said.contains("not found") || said.contains("does not exist")) =>
        {
            ai_error("ai_model", "That AI model isn't available any more.")
        }
        _ => ai_error("ai_failed", "The AI service had a problem."),
    }
}

const API_VERSION: &str = "2026-10-21";
pub(crate) const AI_TIMEOUT: Duration = Duration::from_secs(60);
/// Words that only join a search sentence up: "emails from Priya in October".
const LINKING_WORDS: &[&str] = &[
    "email", "emails", "mail", "mails", "message", "messages", "from", "by", "with", "to", "sent",
    "in", "on", "of", "about", "the", "a", "an", "and", "me", "my", "find", "show",
];
/// Words that only said when, once the dates are worked out.
const TIME_WORDS: &[&str] = &[
    "today",
    "yesterday",
    "tomorrow",
    "tonight",
    "morning",
    "this",
    "last",
    "next",
    "past",
    "week",
    "month",
    "since",
    "day",
    "days",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "jan",
    "february",
    "feb",
    "march",
    "mar",
    "april",
    "apr",
    "may",
    "june",
    "jun",
    "july",
    "jul",
    "august",
    "aug",
    "september",
    "sept",
    "sep",
    "october",
    "oct",
    "november",
    "nov",
    "december",
    "dec",
];

pub(crate) fn ai_error(code: &str, message: &str) -> CoreError {
    CoreError::Ai {
        code: code.into(),
        message: message.into(),
    }
}

impl Engine {
    fn ai_service(&self) -> Result<&'static Service> {
        let id = self.setting("ai_service")?;
        Ok(SERVICES.iter().find(|s| s.id == id).unwrap_or(&SERVICES[0]))
    }

    fn ai_picks(&self) -> Result<Picks> {
        Ok(self
            .store
            .setting(AI_PICKS)?
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default())
    }

    fn save_picks(&self, picks: &Picks) -> Result<()> {
        let text = serde_json::to_string(picks).unwrap_or_default();
        self.store.set_setting(AI_PICKS, &text)?;
        Ok(())
    }

    /// What AI features use to tell whether they can run, and to get the key if there is one.
    pub(crate) fn ai_credential(&self) -> Result<Option<String>> {
        if self.ai_plan()?.is_some() {
            return Ok(Some(PLAN_NOT_A_KEY.to_string()));
        }
        if let Some(key) = self.secrets.get(AI_KEY)?.or_else(|| self.included_key()) {
            return Ok(Some(key));
        }
        Ok(self
            .on_device_in_use()?
            .then(|| crate::on_device::ON_DEVICE_NOT_A_KEY.to_string()))
    }

    /// Ask AI's own: Apple Intelligence when it's picked in Ask AI, otherwise the same as the rest.
    pub(crate) fn ask_ai_credential(&self) -> Result<Option<String>> {
        if self.setting("ask_ai_apple")? == "on"
            && self.on_device_state() == Some(crate::on_device::OnDeviceState::Ready)
        {
            return Ok(Some(crate::on_device::ON_DEVICE_NOT_A_KEY.to_string()));
        }
        self.ai_credential()
    }

    pub(crate) fn save_plan_picks(&self, plan: crate::plans::Plan) -> Result<()> {
        let models = if plan == crate::plans::Plan::Claude {
            crate::plans::CLAUDE_MODELS
                .iter()
                .map(|m| m.to_string())
                .collect()
        } else {
            Vec::new()
        };
        self.save_picks(&Picks {
            models,
            ..Default::default()
        })
    }

    /// Small jobs take the quick one. Writing takes the person's choice, or a good writer.
    pub(crate) fn plan_model(&self, writing: bool) -> Result<String> {
        let picks = self.ai_picks()?;
        Ok(if !writing {
            "haiku".to_string()
        } else if picks.chosen.is_empty() {
            "sonnet".to_string()
        } else {
            picks.chosen
        })
    }

    pub(crate) fn ai_status(&self) -> Result<Value> {
        let key = self.secrets.get(AI_KEY)?;
        let plan = self.ai_plan()?;
        let ending = key
            .as_deref()
            .map(|k| k.chars().rev().take(4).collect::<Vec<_>>())
            .map(|c| c.into_iter().rev().collect::<String>());
        let service = self.ai_service()?;
        let picks = self.ai_picks()?;
        let on_device = self.on_device_in_use()?;
        let services: Vec<Value> = SERVICES
            .iter()
            .map(|s| json!({ "id": s.id, "name": s.name, "keys_page": s.keys_page }))
            .collect();
        Ok(json!({
            "has_key": key.is_some() || plan.is_some() || self.included_ai.is_some() || on_device,
            "on_device": self.on_device_state().map(|s| s.id()),
            "using_on_device": on_device,
            "ask_on_device": self.ask_ai_credential()?.as_deref() == Some(crate::on_device::ON_DEVICE_NOT_A_KEY),
            "included": self.on_included_ai(),
            "via": self.setting("ai_via")?,
            "ending": ending,
            "service": service.id,
            "services": services,
            "models": picks.models,
            "model": picks.chosen,
        }))
    }

    /// What this service offers right now. None if it won't say, which isn't a bad key.
    async fn list_models(&self, service: &Service, key: &str) -> Result<Option<Vec<String>>> {
        let reply = match service.style {
            Style::Anthropic => self
                .ai_http
                .get(format!("{}/v1/models?limit=100", self.ai_base(service)))
                .header("x-api-key", key)
                .header("anthropic-version", API_VERSION),
            Style::OpenAi => self
                .ai_http
                .get(format!("{}/models", self.ai_base(service)))
                .bearer_auth(key),
        }
        .send()
        .await
        .map_err(|_| ai_error("ai_network", "Couldn't reach the AI service."))?;
        let status = reply.status().as_u16();
        if status == 401 || status == 403 {
            return Err(failure(status, ""));
        }
        if status != 200 {
            return Ok(None);
        }
        let value: Value = reply.json().await.unwrap_or(Value::Null);
        // Google's own shape uses `models`, with the rest using `data`.
        let listed = value["data"].as_array().or(value["models"].as_array());
        Ok(listed.map(|list| {
            usable_models(
                list.iter()
                    .filter_map(|m| m["id"].as_str().or(m["name"].as_str()))
                    .map(str::to_string)
                    .collect(),
            )
        }))
    }

    async fn work_out_picks(&self, service: &Service, key: &str, chosen: String) -> Result<Picks> {
        let models = self.list_models(service, key).await?.unwrap_or_default();
        Ok(Picks {
            fast: pick_model(&models, service.fast).unwrap_or_default(),
            chosen: if models.contains(&chosen) {
                chosen
            } else {
                String::new()
            },
            models,
        })
    }

    /// The address calls go to. Tests point this at a stand-in.
    fn ai_base(&self, service: &Service) -> String {
        if self.ai_url == AI_URL {
            service.url.to_string()
        } else {
            self.ai_url.clone()
        }
    }

    /// Saves a key once it's been shown to work with that service.
    pub(crate) async fn ai_set_key(&self, key: &str, service: Option<&str>) -> Result<Value> {
        let key = key.trim();
        if key.is_empty() {
            return Err(ai_error("ai_key", "Paste your key first."));
        }
        let service = match service {
            Some(id) => SERVICES
                .iter()
                .find(|s| s.id == id)
                .ok_or_else(|| CoreError::Invalid("Bad request.".into()))?,
            None => self.ai_service()?,
        };
        let picks = self.work_out_picks(service, key, String::new()).await?;
        if service.style == Style::OpenAi && picks.fast.is_empty() {
            return Err(ai_error(
                "ai_failed",
                "That service didn't list any models this key can use.",
            ));
        }
        self.send_to_ai(
            service,
            fast_model(service, &picks),
            key,
            crate::question::FALLBACK_SYSTEM,
            "emails from Ana",
            crate::question::fallback_schema(),
            // Trying the person's own key.
            false,
        )
        .await?;
        self.secrets.set(AI_KEY, key)?;
        self.store.set_setting("ai_via", crate::plans::VIA_KEY)?;
        self.store.set_setting("ai_service", service.id)?;
        self.save_picks(&picks)?;
        self.settings_changed().await?;
        self.ai_status()
    }

    /// Picks the model for writing and answering. Empty leaves it to the app.
    pub(crate) async fn ai_choose_model(&self, model: &str) -> Result<Value> {
        let mut picks = self.ai_picks()?;
        if !model.is_empty() && !picks.models.iter().any(|m| m == model) {
            return Err(CoreError::Invalid("Bad request.".into()));
        }
        picks.chosen = model.to_string();
        self.save_picks(&picks)?;
        self.settings_changed().await?;
        self.ai_status()
    }

    pub(crate) async fn ai_remove_key(&self) -> Result<Value> {
        // Leaving a plan doesn't throw away a key saved earlier.
        if self.ai_plan()?.is_some() {
            self.store.set_setting("ai_via", crate::plans::VIA_KEY)?;
            self.store.delete_setting(AI_PICKS)?;
            self.settings_changed().await?;
            return self.ai_status();
        }
        self.secrets.delete(AI_KEY)?;
        self.store.delete_setting(AI_PICKS)?;
        self.settings_changed().await?;
        self.ai_status()
    }

    /// What a search means, read here by code: the search box never asks AI. Search syntax
    /// (from:, quotes) is taken as typed. Otherwise people and dates are read from the words, and
    /// every other word typed must be in the email.
    pub(crate) async fn search_understand(
        &self,
        text: &str,
        _today: &str,
        utc_offset_minutes: i64,
    ) -> Result<Value> {
        let plain = parse_plain(text);
        if text.contains(':') || text.contains('"') {
            return Ok(json!({ "used_ai": false, "filters": plain }));
        }
        let zone = chrono::FixedOffset::east_opt((utc_offset_minutes * 60) as i32)
            .or_else(|| chrono::FixedOffset::east_opt(0))
            .expect("UTC is a zone");
        let now = chrono::Utc::now().with_timezone(&zone);
        let (q, people, _) = self.understand(text, now, false).await?;
        let found: Vec<&crate::people::Resolved> = people
            .iter()
            .filter(|p| !p.addresses.is_empty() || !p.domains.is_empty())
            .collect();
        let dated = q.after.is_some() || q.before.is_some();
        if found.is_empty() && !dated {
            return Ok(json!({ "used_ai": false, "filters": plain }));
        }
        let fold = crate::search::fold;
        let named: Vec<String> = found
            .iter()
            .flat_map(|p| p.name.split_whitespace().map(fold))
            .collect();
        // The words typed, less those that named someone or a time, or only joined them up.
        let words: Vec<String> = plain
            .words
            .iter()
            .filter(|w| {
                let f = fold(w);
                let f = f.trim_end_matches("'s").trim_end_matches("’s");
                !named.iter().any(|n| n == f)
                    && !LINKING_WORDS.contains(&f)
                    && !(dated
                        && (TIME_WORDS.contains(&f) || f.chars().all(|c| c.is_ascii_digit())))
            })
            .cloned()
            .collect();
        let mut summary: Vec<String> = found.iter().map(|p| format!("From {}", p.name)).collect();
        if let (Some(after), Some(before)) = (q.after, q.before) {
            let day = |t: i64| {
                chrono::DateTime::from_timestamp(t, 0)
                    .map(|d| d.with_timezone(&zone).format("%-d %b").to_string())
                    .unwrap_or_default()
            };
            let last = day(before - 1);
            let first = day(after);
            summary.push(if first == last {
                first
            } else {
                format!("{first} – {last}")
            });
        }
        summary.extend(words.iter().map(|w| format!("'{w}'")));
        let filters = SearchFilters {
            words,
            from: found
                .iter()
                .flat_map(|p| p.addresses.iter().chain(&p.domains).cloned())
                .collect(),
            after: q.after,
            before: q.before,
            ..plain
        };
        Ok(json!({ "used_ai": false, "filters": filters, "summary": summary.join(" · ") }))
    }

    /// One question for a small job, answered in the given shape.
    pub(crate) async fn ask_ai(
        &self,
        key: &str,
        system: &str,
        question: &str,
        schema: Value,
    ) -> Result<String> {
        self.ask_model(false, key, system, question, schema).await
    }

    /// For writing and answering, where the person can choose the model.
    pub(crate) async fn ask_writer(
        &self,
        key: &str,
        system: &str,
        question: &str,
        schema: Value,
    ) -> Result<String> {
        self.ask_model(true, key, system, question, schema).await
    }

    async fn ask_model(
        &self,
        writing: bool,
        key: &str,
        system: &str,
        question: &str,
        schema: Value,
    ) -> Result<String> {
        if key == crate::on_device::ON_DEVICE_NOT_A_KEY {
            return self.ask_on_device(system, question, schema).await;
        }
        if let Some(plan) = self.ai_plan()? {
            let system = format!(
                "{system}\n\nReply with only a JSON object matching this JSON schema, with no other text: {schema}"
            );
            return self.ask_plan(plan, writing, &system, question).await;
        }
        // AI included with Cloud is always ours, with the usual models, within its budget.
        // Decided once, here, by whose key this request really carries, and kept to the
        // end, so a key set or removed while it's on its way can't change whose it was.
        // A key that's also the person's own is theirs.
        let included = self.included_key().is_some_and(|k| k == key)
            && self.secrets.get(AI_KEY)?.as_deref() != Some(key);
        let service = if included {
            &SERVICES[0]
        } else {
            self.ai_service()?
        };
        let picks = if included {
            Picks::default()
        } else {
            self.ai_picks()?
        };
        // On included AI, what this one may cost is set aside before it goes, so requests
        // running side by side can't each find room under the limit.
        let (writing, _held) = if included {
            let rough = (system.len() + question.len() + schema.to_string().len()) as i64;
            let (writing, held) = self.included_gate(writing, |writing| {
                let model = model_for(service, &picks, writing);
                crate::allowance::rough_cost(&model, rough)
            })?;
            (writing, Some(held))
        } else {
            (writing, None)
        };
        let model = model_for(service, &picks, writing);
        match self
            .send_to_ai(
                service,
                &model,
                key,
                system,
                question,
                schema.clone(),
                included,
            )
            .await
        {
            // Services retire models often. Find out what's there now and try once more.
            Err(CoreError::Ai { code, .. }) if code == "ai_model" => {
                let picks = self.work_out_picks(service, key, picks.chosen).await?;
                // Included AI's models are its own: the person's choices stay as they were.
                if !included {
                    self.save_picks(&picks)?;
                }
                let model = model_for(service, &picks, writing);
                self.send_to_ai(service, &model, key, system, question, schema, included)
                    .await
            }
            other => other,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_to_ai(
        &self,
        service: &Service,
        model: &str,
        key: &str,
        system: &str,
        question: &str,
        schema: Value,
        included: bool,
    ) -> Result<String> {
        match service.style {
            Style::Anthropic => {
                self.send_anthropic(service, model, key, system, question, schema, included)
                    .await
            }
            Style::OpenAi => {
                self.send_openai_style(service, model, key, system, question, schema, included)
                    .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_anthropic(
        &self,
        service: &Service,
        model: &str,
        key: &str,
        system: &str,
        question: &str,
        schema: Value,
        included: bool,
    ) -> Result<String> {
        let body = json!({
            "model": model,
            "max_tokens": MOST_ANSWER_TOKENS,
            // The instructions, style profile included, are the same each time, so the AI
            // company can reuse them at a lower price when they're long enough to count.
            "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
            "messages": [{ "role": "user", "content": question }],
            "output_config": { "format": { "type": "json_schema", "schema": schema } },
        });
        let reply = self
            .ai_http
            .post(format!("{}/v1/messages", self.ai_base(service)))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|_| ai_error("ai_network", "Couldn't reach the AI service."))?;
        let status = reply.status().as_u16();
        let value: Value = reply.json().await.unwrap_or(Value::Null);
        if status != 200 {
            return Err(failure(
                status,
                value["error"]["message"].as_str().unwrap_or(""),
            ));
        }
        let usage = &value["usage"];
        self.note_ai_usage(
            crate::allowance::Usage {
                input: usage["input_tokens"].as_i64().unwrap_or(0),
                output: usage["output_tokens"].as_i64().unwrap_or(0),
                cache_read: usage["cache_read_input_tokens"].as_i64().unwrap_or(0),
                cache_write: usage["cache_creation_input_tokens"].as_i64().unwrap_or(0),
            },
            model,
            included,
        );
        if value["stop_reason"] != "end_turn" {
            eprintln!("AI answer stopped early: {}", value["stop_reason"]);
            return Err(ai_error("ai_failed", "The AI couldn't work that one out."));
        }
        Ok(value["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
            .and_then(|b| b["text"].as_str())
            .unwrap_or("")
            .to_string())
    }

    /// Not every service here can follow a schema, so it goes in the instructions, and
    /// asking for JSON is dropped for the few models that refuse to be asked.
    #[allow(clippy::too_many_arguments)]
    async fn send_openai_style(
        &self,
        service: &Service,
        model: &str,
        key: &str,
        system: &str,
        question: &str,
        schema: Value,
        included: bool,
    ) -> Result<String> {
        let system = format!(
            "{system}\n\nReply with only a JSON object matching this JSON schema, with no other text:\n{schema}"
        );
        // OpenAI's newer models only take the longer name for this.
        let limit = if service.id == "openai" {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        let mut body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": question },
            ],
            "response_format": { "type": "json_object" },
        });
        body[limit] = json!(MOST_ANSWER_TOKENS);
        let mut value = Value::Null;
        for attempt in 0..2 {
            let reply = self
                .ai_http
                .post(format!("{}/chat/completions", self.ai_base(service)))
                .bearer_auth(key)
                .json(&body)
                .send()
                .await
                .map_err(|_| ai_error("ai_network", "Couldn't reach the AI service."))?;
            let status = reply.status().as_u16();
            value = reply.json().await.unwrap_or(Value::Null);
            if status == 200 {
                break;
            }
            let said = value["error"]["message"].as_str().unwrap_or("").to_string();
            if attempt == 0 && matches!(status, 400 | 422) && said.contains("response_format") {
                body.as_object_mut().map(|b| b.remove("response_format"));
                continue;
            }
            return Err(failure(status, &said));
        }
        self.note_ai_usage(
            crate::allowance::Usage {
                input: value["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
                output: value["usage"]["completion_tokens"].as_i64().unwrap_or(0),
                ..Default::default()
            },
            model,
            included,
        );
        let choice = &value["choices"][0];
        if choice["finish_reason"] != "stop" {
            eprintln!("AI answer stopped early: {}", choice["finish_reason"]);
            return Err(ai_error("ai_failed", "The AI couldn't work that one out."));
        }
        Ok(without_code_fence(choice["message"]["content"].as_str().unwrap_or("")).to_string())
    }
}

/// For small jobs.
fn fast_model<'a>(service: &Service, picks: &'a Picks) -> &'a str {
    if !picks.fast.is_empty() {
        return &picks.fast;
    }
    // Claude's list isn't needed to get going.
    if service.style == Style::Anthropic {
        crate::assist::FAST_MODEL
    } else {
        ""
    }
}

/// The person's choice for writing, or otherwise a balanced one. Small jobs take the quick one.
fn model_for(service: &Service, picks: &Picks, writing: bool) -> String {
    if writing && !picks.chosen.is_empty() {
        return picks.chosen.clone();
    }
    if writing {
        let matching = |id: &&String| {
            service
                .balanced
                .iter()
                .any(|w| id.to_lowercase().contains(w))
        };
        if picks.models.iter().any(|id| matching(&id)) {
            if let Some(found) = pick_model(&picks.models, service.balanced) {
                return found;
            }
        }
        // Claude's list isn't needed to get going.
        if picks.models.is_empty() && service.style == Style::Anthropic {
            return BALANCED_CLAUDE.to_string();
        }
    }
    fast_model(service, picks).to_string()
}

const BALANCED_CLAUDE: &str = "claude-sonnet-5-5";

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        usable_models(list.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn automatic_writes_with_a_balanced_model_and_sorts_with_the_quick_one() {
        let claude = &SERVICES[0];
        let picks = Picks {
            fast: "claude-haiku-5-5".into(),
            chosen: String::new(),
            models: ids(&["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"]),
        };
        assert_eq!(model_for(claude, &picks, true), "claude-sonnet-5-5");
        assert_eq!(model_for(claude, &picks, false), "claude-haiku-5-5");
        let chosen = Picks {
            chosen: "claude-opus-5-5".into(),
            ..picks
        };
        assert_eq!(model_for(claude, &chosen, true), "claude-opus-5-5");
        let unknown = Picks::default();
        assert_eq!(model_for(claude, &unknown, true), BALANCED_CLAUDE);
    }

    #[test]
    fn models_that_do_not_write_are_left_out() {
        let found = ids(&[
            "gpt-5-luna",
            "whisper-1",
            "text-embedding-3",
            "gpt-image-2",
            "omni-moderation",
        ]);
        assert_eq!(found, vec!["gpt-5-luna"]);
    }

    #[test]
    fn the_newest_of_the_right_kind_is_picked() {
        let m = ids(&[
            "gpt-5.4-nano",
            "gpt-5.6-luna",
            "gpt-6.1-sol",
            "gpt-5.5-luna",
        ]);
        assert_eq!(pick_model(&m, &["luna", "nano"]).unwrap(), "gpt-5.6-luna");
        assert_eq!(pick_model(&m, &["sol"]).unwrap(), "gpt-6.1-sol");
    }

    #[test]
    fn google_names_are_tidied_and_flash_means_full_flash() {
        let m = ids(&[
            "models/gemini-3.8-flash-lite",
            "models/gemini-3.8-flash",
            "models/gemini-3.1-pro-preview",
        ]);
        assert_eq!(pick_model(&m, &["flash"]).unwrap(), "gemini-3.8-flash");
        assert_eq!(pick_model(&m, &["pro"]).unwrap(), "gemini-3.1-pro-preview");
    }

    #[test]
    fn something_is_picked_even_if_no_word_matches() {
        let m = ids(&["odd-model-1", "odd-model-2"]);
        assert_eq!(pick_model(&m, &["luna"]).unwrap(), "odd-model-2");
        assert!(pick_model(&[], &["luna"]).is_none());
    }

    #[test]
    fn a_code_fence_around_the_answer_comes_off() {
        assert_eq!(without_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(without_code_fence(" {\"a\":1} "), "{\"a\":1}");
        assert_eq!(
            without_code_fence("Here is the reply:\n```json\n{\"a\":1}\n```\nHope it helps."),
            "{\"a\":1}"
        );
        assert_eq!(
            without_code_fence("Sure. {\"a\":\"x}\"} Done."),
            "{\"a\":\"x}\"}"
        );
        assert_eq!(without_code_fence("no json here"), "no json here");
    }

    #[test]
    fn every_service_is_in_the_settings_list() {
        let listed: Vec<&str> = SERVICES.iter().map(|s| s.id).collect();
        assert_eq!(listed, AI_SERVICE_IDS);
    }
}
