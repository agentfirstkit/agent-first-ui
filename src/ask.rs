//! A small AFUI-owned interaction, independent of any Provider's domain.
//!
//! A request supplies text, optional context blocks, and one or more questions
//! with stable option IDs. The standard page collects one answer per question;
//! it never executes an answer or interprets its domain meaning.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use axum::{Router, http::header, response::Html, routing::get};
use serde::{Deserialize, Serialize};

use crate::{
    ActiveUiDelivery, Error, Outcome, Result, UiAppIcon, UiCallError, UiCompletion, UiCspNonce,
    UiDeliveryFacts, UiDeliveryPlan, UiPagePolicy, UiPageScript, UiSession, UiSessionRuntime,
    page_kernel_source,
};

const MAX_QUESTIONS: usize = 8;
const MAX_OPTIONS: usize = 32;
const MAX_QUESTION_BYTES: usize = 8 * 1024;
const MAX_LABEL_BYTES: usize = 1024;
const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;
const MAX_CONTEXT_BLOCKS: usize = 32;
const MAX_BLOCK_BYTES: usize = 8 * 1024;
const MAX_LIST_ITEMS: usize = 32;
const MAX_ITEM_BYTES: usize = 1024;
const MAX_TEXT_BYTES: usize = 16 * 1024;
/// A registry subject is an identity line in a session list, not the question.
const MAX_SUBJECT_CHARS: usize = 120;

/// One offered answer. The ID, rather than the label or position, identifies it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Marks the asker's suggestion. Shown as a badge; never preselected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recommended: bool,
}

/// One question and the answers it offers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,
    /// Allow choosing several options instead of at most one.
    #[serde(default)]
    pub multiple: bool,
    /// Allow the person's own words: a written answer, and a note on each
    /// chosen option.
    #[serde(default = "allow_custom")]
    pub allow_custom: bool,
}

fn allow_custom() -> bool {
    true
}

/// Background the person needs before answering. Typed blocks, rendered by the
/// page as text; never Markdown or HTML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AskBlock {
    Paragraph {
        text: String,
    },
    List {
        items: Vec<String>,
    },
    Code {
        text: String,
    },
    /// A local raster image, copied into a session-scoped blob before readiness.
    Image {
        path: PathBuf,
        title: String,
    },
    /// Plain-text cells; rows must match the number of column headings.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Read-only unified diff text; no patch is ever executed.
    Diff {
        text: String,
    },
}

/// Everything a standard question page displays.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<AskBlock>,
    pub questions: Vec<AskQuestion>,
}

/// One option the page submits as chosen, with the person's note on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskChoice {
    pub option_id: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// What the page submits for one question.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskReply {
    pub question_id: String,
    #[serde(default)]
    pub choices: Vec<AskChoice>,
    #[serde(default)]
    pub text: Option<String>,
}

/// What the page submits through AFUI's normal typed runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AskAction {
    Submit { answers: Vec<AskReply> },
    Cancel,
}

/// One chosen option, with its server-resolved label and the person's note
/// on that choice, if they wrote one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskSelection {
    pub option_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The validated answer to one question.
///
/// `text` is a written answer of the person's own. A single-choice question
/// has either a selection or `text`; a multiple-choice one may have both,
/// `text` then standing for an answer the options did not cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub question_id: String,
    pub selected: Vec<AskSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Every question answered, or an explicit cancellation without answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AskResponse {
    Answered { answers: Vec<AskAnswer> },
    Cancelled,
}

/// A question that is reachable and can report readiness before waiting.
pub struct ActiveAsk {
    delivery: ActiveUiDelivery<AskResponse>,
    completion: UiCompletion<AskResponse>,
    runtime: UiSessionRuntime<AskAction, (), serde_json::Value>,
    request: AskRequest,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn has_text(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max
}

impl AskRequest {
    /// One question, the common case.
    #[must_use]
    pub fn single(question: AskQuestion) -> Self {
        Self {
            context: Vec::new(),
            questions: vec![question],
        }
    }

    /// Validate everything before a credential is issued or a window opens.
    pub fn validate(&self) -> Result<()> {
        self.validate_fields()?;
        self.images()?;
        Ok(())
    }

    fn validate_fields(&self) -> Result<()> {
        let reject = |reason| Err(Error::InvalidAsk { reason });
        if self.context.len() > MAX_CONTEXT_BLOCKS {
            return reject("supply at most 32 context blocks");
        }
        for block in &self.context {
            let valid = match block {
                AskBlock::Paragraph { text } | AskBlock::Code { text } => {
                    has_text(text, MAX_BLOCK_BYTES)
                }
                AskBlock::Image { title, .. } => has_text(title, MAX_LABEL_BYTES),
                AskBlock::Diff { text } => has_text(text, 64 * 1024),
                AskBlock::Table { columns, rows } => {
                    !columns.is_empty()
                        && columns.len() <= 32
                        && rows.len() <= 128
                        && columns.iter().all(|cell| has_text(cell, MAX_ITEM_BYTES))
                        && rows.iter().all(|row| {
                            row.len() == columns.len()
                                && row.iter().all(|cell| cell.len() <= MAX_ITEM_BYTES)
                        })
                        && columns
                            .iter()
                            .chain(rows.iter().flatten())
                            .map(String::len)
                            .sum::<usize>()
                            <= 256 * 1024
                }
                AskBlock::List { items } => {
                    !items.is_empty()
                        && items.len() <= MAX_LIST_ITEMS
                        && items.iter().all(|item| has_text(item, MAX_ITEM_BYTES))
                }
            };
            if !valid {
                return reject(
                    "invalid context block: check text, image title, rectangular table and documented size limits",
                );
            }
        }
        if self.questions.is_empty() || self.questions.len() > MAX_QUESTIONS {
            return reject("supply between 1 and 8 questions");
        }
        let mut question_ids = BTreeSet::new();
        for question in &self.questions {
            if !valid_id(&question.id) || !question_ids.insert(&question.id) {
                return reject(
                    "question IDs must be unique, 1–64 ASCII letters, digits, underscores or hyphens",
                );
            }
            if !has_text(&question.question, MAX_QUESTION_BYTES) {
                return reject("each question must contain text and be at most 8192 bytes");
            }
            if question.options.len() > MAX_OPTIONS {
                return reject("supply at most 32 options per question");
            }
            if question.options.is_empty() && !question.allow_custom {
                return reject("a question without options requires allow_custom=true");
            }
            let mut option_ids = BTreeSet::new();
            for option in &question.options {
                if !valid_id(&option.id) || !option_ids.insert(&option.id) {
                    return reject(
                        "option IDs must be unique within a question, 1–64 ASCII letters, digits, underscores or hyphens",
                    );
                }
                if !has_text(&option.label, MAX_LABEL_BYTES) {
                    return reject("each option label must contain text and be at most 1024 bytes");
                }
                if option
                    .description
                    .as_ref()
                    .is_some_and(|text| text.len() > MAX_DESCRIPTION_BYTES)
                {
                    return reject("option descriptions must be at most 4096 bytes");
                }
            }
            if question.options.iter().filter(|o| o.recommended).count() > 1 {
                return reject("recommend at most one option per question");
            }
        }
        Ok(())
    }

    fn images(&self) -> Result<Vec<(usize, &'static str, Vec<u8>)>> {
        let mut images = Vec::new();
        let mut total_bytes = 0;
        for (index, block) in self.context.iter().enumerate() {
            if let AskBlock::Image { path, .. } = block {
                let (media_type, bytes) = read_image(path)?;
                total_bytes += bytes.len();
                if images.len() >= 16 || total_bytes > 32 * 1024 * 1024 {
                    return Err(Error::InvalidAsk {
                        reason: "supply at most 16 images totalling at most 32 MiB",
                    });
                }
                images.push((index, media_type, bytes));
            }
        }
        Ok(images)
    }

    /// The registry subject: the first question's first line, kept short.
    fn subject(&self) -> String {
        let line = self
            .questions
            .first()
            .and_then(|question| question.question.lines().find(|l| !l.trim().is_empty()))
            .unwrap_or_default()
            .trim();
        let mut indices = line.char_indices();
        let Some((end, _)) = indices.nth(MAX_SUBJECT_CHARS - 1) else {
            return line.to_owned();
        };
        if indices.next().is_none() {
            return line.to_owned();
        }
        format!("{}…", &line[..end])
    }

    fn answer(&self, action: &AskAction) -> std::result::Result<AskResponse, UiCallError> {
        let replies = match action {
            AskAction::Cancel => return Ok(AskResponse::Cancelled),
            AskAction::Submit { answers } => answers,
        };
        let mut seen = BTreeSet::new();
        for reply in replies {
            if !self.questions.iter().any(|q| q.id == reply.question_id)
                || !seen.insert(&reply.question_id)
            {
                return Err(UiCallError::new(
                    "ask_question_invalid",
                    "Answer each offered question once.",
                ));
            }
        }
        let mut answers = Vec::with_capacity(self.questions.len());
        for question in &self.questions {
            let Some(reply) = replies.iter().find(|r| r.question_id == question.id) else {
                return Err(UiCallError::new(
                    "ask_answer_missing",
                    "Answer every question: choose an option or write an answer.",
                ));
            };
            answers.push(question.answer(reply)?);
        }
        Ok(AskResponse::Answered { answers })
    }

    /// Open the standard page through the same delivery plan as custom pages.
    pub async fn start(self, plan: UiDeliveryPlan) -> Result<ActiveAsk> {
        self.validate_fields()?;
        let images = self.images()?;
        let nonce = UiCspNonce::generate()?;
        let policy = UiPagePolicy::new(UiPageScript::Nonce(nonce.clone()))
            .allow_images()
            .allow_runtime();
        let (session, runtime) = UiSession::<AskResponse>::new("afui", "ask")?
            .with_subject(self.subject())
            .with_app_icon(UiAppIcon::from_svg(include_str!(
                "serve/assets/app-icon.svg"
            ))?)
            .with_security_policy(policy.into_security_policy())
            .with_runtime_and_blobs::<AskAction, (), serde_json::Value>(image_policy())?;
        let completion = session.completion();
        let mut state = serde_json::to_value(&self).map_err(Error::SessionRuntimeEncode)?;
        if let Some(blocks) = state
            .get_mut("context")
            .and_then(serde_json::Value::as_array_mut)
        {
            for (index, media_type, bytes) in images {
                let reference = runtime.store_blob(media_type, bytes)?;
                let title = blocks[index]["title"].clone();
                blocks[index] =
                    serde_json::json!({"type": "image", "title": title, "blob": reference});
            }
        }
        // Large evidence stays behind this session's credential rather than
        // widening the runtime frame limit shared by every kind of page.
        match runtime.publish_state(&state) {
            Ok(_) => {}
            Err(Error::SessionRuntimeMessageTooLarge { .. }) => {
                runtime.publish_state(&serde_json::json!({"request_url": "ask-request.json"}))?;
            }
            Err(error) => return Err(error),
        }
        let request_json = serde_json::to_vec(&state).map_err(Error::SessionRuntimeEncode)?;
        let page = include_str!("ask/page.html").replace(
            "<!-- afui:trusted-runtime -->",
            &format!(
                "<script nonce=\"{}\">(function(){{{}\n{}}})();</script>",
                nonce.as_str(),
                page_kernel_source(),
                include_str!("ask/page.js")
            ),
        );
        let router = Router::new()
            .route(
                "/ask-request.json",
                get(move || {
                    let bytes = request_json.clone();
                    async move {
                        (
                            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                            bytes,
                        )
                    }
                }),
            )
            .route(
                "/",
                get(move || {
                    let page = page.clone();
                    async move { Html(page) }
                }),
            )
            .route(
                "/ask.css",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                        include_str!("ask/page.css"),
                    )
                }),
            );
        let delivery = plan.start(session, router).await?;
        Ok(ActiveAsk {
            delivery,
            completion,
            runtime,
            request: self,
        })
    }
}

const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

fn image_policy() -> crate::UiBlobPolicy {
    crate::UiBlobPolicy::new(MAX_IMAGE_BYTES, ["image/png", "image/jpeg", "image/webp"])
}

fn read_image(path: &Path) -> Result<(&'static str, Vec<u8>)> {
    let invalid = || Error::InvalidAsk {
        reason: "image must be a local PNG, JPEG or WebP file of at most 8 MiB",
    };
    if !std::fs::metadata(path).map_err(|_| invalid())?.is_file() {
        return Err(invalid());
    }
    let file = std::fs::File::open(path).map_err(|_| invalid())?;
    if !file.metadata().map_err(|_| invalid())?.is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    let media_type = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        Some("jpg" | "jpeg") if bytes.starts_with(b"\xff\xd8\xff") => "image/jpeg",
        Some("webp") if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => {
            "image/webp"
        }
        _ => return Err(invalid()),
    };
    if bytes.len() as u64 > MAX_IMAGE_BYTES || !image_policy().admits(media_type) {
        return Err(invalid());
    }
    Ok((media_type, bytes))
}

/// The person's own words, or nothing if they wrote only whitespace.
fn own_words(text: Option<&String>) -> std::result::Result<Option<String>, UiCallError> {
    if text.is_some_and(|text| text.len() > MAX_TEXT_BYTES) {
        return Err(UiCallError::new(
            "ask_text_invalid",
            "Written answers and notes must be at most 16384 bytes.",
        ));
    }
    Ok(text.filter(|text| !text.trim().is_empty()).cloned())
}

impl AskQuestion {
    fn answer(&self, reply: &AskReply) -> std::result::Result<AskAnswer, UiCallError> {
        let mut chosen = BTreeSet::new();
        let mut selected = Vec::with_capacity(reply.choices.len());
        for choice in &reply.choices {
            let option = self
                .options
                .iter()
                .find(|option| option.id == choice.option_id)
                .filter(|_| chosen.insert(&choice.option_id))
                .ok_or_else(|| {
                    UiCallError::new("ask_option_invalid", "Choose among the offered options.")
                })?;
            selected.push(AskSelection {
                option_id: option.id.clone(),
                label: option.label.clone(),
                note: own_words(choice.note.as_ref())?,
            });
        }
        let text = own_words(reply.text.as_ref())?;
        if !self.allow_custom && (text.is_some() || selected.iter().any(|s| s.note.is_some())) {
            return Err(UiCallError::new(
                "ask_custom_disabled",
                "Written answers and notes are not available for this question.",
            ));
        }
        if !self.multiple && selected.len() + usize::from(text.is_some()) > 1 {
            return Err(UiCallError::new(
                "ask_option_invalid",
                "Choose one option or write your own answer, not both.",
            ));
        }
        if selected.is_empty() && text.is_none() {
            return Err(UiCallError::new(
                "ask_answer_missing",
                "Answer every question: choose an option or write an answer.",
            ));
        }
        Ok(AskAnswer {
            question_id: self.id.clone(),
            selected,
            text,
        })
    }
}

impl ActiveAsk {
    /// Readiness facts; local access credentials stay private.
    #[must_use]
    pub fn facts(&self) -> UiDeliveryFacts {
        self.delivery.facts()
    }

    /// Wait for a valid submission, cancellation, close, or expiry.
    ///
    /// Invalid calls fail without ending the question. No detached worker can
    /// fail while leaving a window waiting: the calls and ending share a loop.
    pub async fn wait(self) -> Result<Outcome<AskResponse>> {
        let ending = self.delivery.wait();
        tokio::pin!(ending);
        loop {
            tokio::select! {
                outcome = &mut ending => return outcome,
                received = self.runtime.recv() => {
                    let Some(call) = received? else { return ending.await; };
                    if call.is_cancelled() { continue; }
                    // A call cancelled, or a session ended, between receiving the
                    // call and answering it is the race this loop exists to
                    // lose gracefully: the next turn sees the cancellation or
                    // the ending, rather than the question failing outright.
                    match self.request.answer(call.action()) {
                        Ok(response) => {
                            let replied = tokio::select! {
                                outcome = &mut ending => return outcome,
                                reply = call.finish_before_session_end(&()) => reply,
                            };
                            match replied {
                                Ok(()) => {
                                    self.completion.complete(response).await;
                                    return ending.await;
                                }
                                Err(Error::SessionRuntimeCallClosed) => {}
                                Err(Error::SessionRuntimeClosed) => return ending.await,
                                Err(error) => return Err(error),
                            }
                        }
                        Err(failure) => match call.fail(failure).await {
                            Ok(()) | Err(Error::SessionRuntimeCallClosed) => {}
                            Err(Error::SessionRuntimeClosed) => return ending.await,
                            Err(error) => return Err(error),
                        },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str) -> AskOption {
        AskOption {
            id: id.into(),
            label: id.to_uppercase(),
            description: None,
            recommended: false,
        }
    }

    fn question(id: &str, multiple: bool) -> AskQuestion {
        AskQuestion {
            id: id.into(),
            question: "Next?".into(),
            options: vec![option("retry"), option("stop")],
            multiple,
            allow_custom: true,
        }
    }

    fn request() -> AskRequest {
        AskRequest::single(question("next", false))
    }

    /// `(question, [(option, note)], written answer)` per reply.
    type Reply<'a> = (&'a str, &'a [(&'a str, Option<&'a str>)], Option<&'a str>);

    fn submit(replies: &[Reply<'_>]) -> AskAction {
        AskAction::Submit {
            answers: replies
                .iter()
                .map(|(id, choices, text)| AskReply {
                    question_id: (*id).into(),
                    choices: choices
                        .iter()
                        .map(|(option, note)| AskChoice {
                            option_id: (*option).into(),
                            note: note.map(str::to_owned),
                        })
                        .collect(),
                    text: text.map(str::to_owned),
                })
                .collect(),
        }
    }

    fn code(result: std::result::Result<AskResponse, UiCallError>) -> String {
        result.unwrap_err().code().to_owned()
    }

    #[test]
    fn validation_rejects_ambiguous_or_unbounded_questions() {
        let good = request();
        assert!(good.validate().is_ok());
        let mut bad = good.clone();
        bad.questions[0].options.push(option("retry"));
        assert!(bad.validate().is_err());
        for text in [" \n".to_owned(), "x".repeat(8193)] {
            bad = good.clone();
            bad.questions[0].question = text;
            assert!(bad.validate().is_err());
        }
        for id in ["", "</script>", "a b"] {
            bad = good.clone();
            bad.questions[0].options[0].id = id.into();
            assert!(bad.validate().is_err());
            bad = good.clone();
            bad.questions[0].id = id.into();
            assert!(bad.validate().is_err());
        }
        bad = good.clone();
        bad.questions.push(question("next", false));
        assert!(bad.validate().is_err(), "duplicate question IDs");
        bad = good.clone();
        bad.questions.clear();
        assert!(bad.validate().is_err());
        bad = good.clone();
        bad.questions[0].options.clear();
        assert!(bad.validate().is_ok());
        bad.questions[0].allow_custom = false;
        assert!(bad.validate().is_err());
        bad = good.clone();
        bad.questions[0].options[0].label = " ".into();
        assert!(bad.validate().is_err());
        bad = good.clone();
        for option in &mut bad.questions[0].options {
            option.recommended = true;
        }
        assert!(bad.validate().is_err(), "two recommendations");
        for block in [
            AskBlock::Paragraph { text: " ".into() },
            AskBlock::List { items: vec![] },
            AskBlock::Code {
                text: "x".repeat(8193),
            },
        ] {
            bad = good.clone();
            bad.context = vec![block];
            assert!(bad.validate().is_err());
        }
        assert!(
            serde_json::from_str::<AskOption>(r#"{"id":"x","label":"X","command":"run"}"#).is_err()
        );
        assert!(serde_json::from_str::<AskBlock>(r#"{"type":"html","text":"<b>"}"#).is_err());
    }

    #[test]
    fn subject_is_the_first_line_kept_short() {
        let mut request = request();
        request.questions[0].question = "\n  第一行  \n第二行".into();
        assert_eq!(request.subject(), "第一行");
        request.questions[0].question = "字".repeat(300);
        let subject = request.subject();
        assert_eq!(subject.chars().count(), MAX_SUBJECT_CHARS);
        assert!(subject.ends_with('…'));
        for character in ["a", "界", "🙂"] {
            let exact = character.repeat(MAX_SUBJECT_CHARS);
            request.questions[0].question = exact.clone();
            assert_eq!(request.subject(), exact);
            request.questions[0].question.push_str(character);
            assert_eq!(
                request.subject(),
                format!("{}…", character.repeat(MAX_SUBJECT_CHARS - 1))
            );
        }
    }

    #[test]
    fn choices_notes_and_written_answers_are_validated_and_preserved() {
        let mut request = request();
        let answered = |request: &AskRequest, action| match request.answer(&action).unwrap() {
            AskResponse::Answered { answers } => answers.into_iter().next().unwrap(),
            AskResponse::Cancelled => unreachable!(),
        };
        let answer = answered(
            &request,
            submit(&[("next", &[("retry", Some("  先看日志 "))], None)]),
        );
        assert_eq!(
            answer.selected,
            vec![AskSelection {
                option_id: "retry".into(),
                label: "RETRY".into(),
                note: Some("  先看日志 ".into()),
            }]
        );
        assert_eq!(answer.text, None);
        let answer = answered(
            &request,
            submit(&[("next", &[("retry", Some(" \n"))], None)]),
        );
        assert_eq!(answer.selected[0].note, None, "a blank note is no note");
        let written = "  另一种办法\n ";
        let answer = answered(&request, submit(&[("next", &[], Some(written))]));
        assert!(answer.selected.is_empty());
        assert_eq!(answer.text.as_deref(), Some(written));

        let invalid = [
            (
                submit(&[("next", &[("forged", None)], None)]),
                "ask_option_invalid",
            ),
            (
                submit(&[("next", &[("retry", None), ("stop", None)], None)]),
                "ask_option_invalid",
            ),
            (
                submit(&[("next", &[("retry", None)], Some("also mine"))]),
                "ask_option_invalid",
            ),
            (submit(&[("next", &[], Some(" "))]), "ask_answer_missing"),
            (submit(&[]), "ask_answer_missing"),
            (
                submit(&[("other", &[("retry", None)], None)]),
                "ask_question_invalid",
            ),
        ];
        for (action, expected) in invalid {
            assert_eq!(code(request.answer(&action)), expected);
        }
        let long = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            code(request.answer(&submit(&[("next", &[], Some(&long))]))),
            "ask_text_invalid"
        );
        assert_eq!(
            code(request.answer(&submit(&[("next", &[("retry", Some(&long))], None)]))),
            "ask_text_invalid"
        );
        request.questions[0].allow_custom = false;
        assert_eq!(
            code(request.answer(&submit(&[("next", &[("retry", Some("note"))], None)]))),
            "ask_custom_disabled"
        );
        assert_eq!(
            request.answer(&AskAction::Cancel).unwrap(),
            AskResponse::Cancelled
        );
    }

    #[test]
    fn multiple_questions_and_multiple_choices_answer_in_question_order() {
        let request = AskRequest {
            context: vec![],
            questions: vec![question("first", false), question("both", true)],
        };
        let action = submit(&[
            (
                "both",
                &[("stop", Some("先这个")), ("retry", None)],
                Some("还有别的"),
            ),
            ("first", &[("stop", None)], None),
        ]);
        let AskResponse::Answered { answers } = request.answer(&action).unwrap() else {
            unreachable!()
        };
        assert_eq!(answers[0].question_id, "first");
        assert_eq!(answers[1].question_id, "both");
        let chosen: Vec<_> = answers[1]
            .selected
            .iter()
            .map(|s| (s.option_id.as_str(), s.note.as_deref()))
            .collect();
        assert_eq!(chosen, [("stop", Some("先这个")), ("retry", None)]);
        assert_eq!(
            answers[1].text.as_deref(),
            Some("还有别的"),
            "an answer beside the choices"
        );
        let duplicate = submit(&[
            ("both", &[("stop", None), ("stop", None)], None),
            ("first", &[("stop", None)], None),
        ]);
        assert_eq!(code(request.answer(&duplicate)), "ask_option_invalid");
        let twice = submit(&[
            ("first", &[("stop", None)], None),
            ("first", &[("retry", None)], None),
        ]);
        assert_eq!(code(request.answer(&twice)), "ask_question_invalid");
        let partial = submit(&[("first", &[("stop", None)], None)]);
        assert_eq!(code(request.answer(&partial)), "ask_answer_missing");
    }
    #[test]
    fn context_validation_rejects_unsafe_images_and_nonrectangular_tables() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("a.png");
        let mut good = request();
        for block in [
            AskBlock::Image {
                path: "https://example.invalid/image.png".into(),
                title: "A".into(),
            },
            AskBlock::Image {
                path: directory.path().join("a.svg"),
                title: "A".into(),
            },
            AskBlock::Table {
                columns: vec!["One".into(), "Two".into()],
                rows: vec![vec!["short".into()]],
            },
            AskBlock::Table {
                columns: vec![],
                rows: vec![],
            },
            AskBlock::Diff {
                text: "x".repeat(64 * 1024 + 1),
            },
        ] {
            good.context = vec![block];
            assert!(good.validate().is_err());
        }
        std::fs::write(&image, "<svg>not a raster</svg>").unwrap();
        good.context = vec![AskBlock::Image {
            path: image.clone(),
            title: "A".into(),
        }];
        assert!(good.validate().is_err());
        let mut large = b"\x89PNG\r\n\x1a\n".to_vec();
        large.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        std::fs::write(&image, &large).unwrap();
        assert!(good.validate().is_err());
        std::fs::write(&image, b"\x89PNG\r\n\x1a\n").unwrap();
        assert!(good.validate().is_ok());
        good.context = vec![good.context[0].clone(); 17];
        assert!(good.validate().is_err());
    }
}
