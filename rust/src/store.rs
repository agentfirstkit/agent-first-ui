use crate::error::AgentFirstUiError;
use crate::request::{PrepareRequestSpec, PreparedRequest, prepare_request};
use crate::runtime::apply_message;
use crate::types::{AgentFirstUiMessage, MessagePayload, Space};

/// A running space state: the current space, the emit-log position, and an audit
/// trail of every message received.
#[derive(Debug, Clone, Default)]
pub struct SpaceStore {
    space: Option<Space>,
    sequence: u64,
    audit: Vec<AgentFirstUiMessage>,
}

impl SpaceStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(space: Space, sequence: u64) -> Self {
        Self {
            space: Some(space),
            sequence,
            audit: Vec::new(),
        }
    }

    pub fn space(&self) -> Option<&Space> {
        self.space.as_ref()
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn audit(&self) -> &[AgentFirstUiMessage] {
        &self.audit
    }

    pub fn replace_space(&mut self, space: Space) {
        self.space = Some(space);
    }

    /// Apply a snapshot or emit message, advancing the store. A request message
    /// returns `UnsupportedMessageType`.
    pub fn apply(&mut self, message: &AgentFirstUiMessage) -> Result<&Space, AgentFirstUiError> {
        if let MessagePayload::Emit { sequence, .. } = &message.payload
            && self.space.is_some()
            && *sequence <= self.sequence
        {
            return Err(AgentFirstUiError::NonMonotonicSequence {
                current: self.sequence,
                incoming: *sequence,
            });
        }
        let (next_space, next_sequence) = apply_message(self.space.as_ref(), message)?;
        self.space = Some(next_space);
        self.sequence = next_sequence;
        self.audit.push(message.clone());
        self.space.as_ref().ok_or(AgentFirstUiError::MissingSpace)
    }

    /// Record a request message in the audit trail without changing space state.
    pub fn record_request(
        &mut self,
        message: AgentFirstUiMessage,
    ) -> Result<(), AgentFirstUiError> {
        message.ensure_version()?;
        match &message.payload {
            MessagePayload::Request { .. } => {
                self.audit.push(message);
                Ok(())
            }
            other => Err(AgentFirstUiError::UnsupportedMessageType {
                message_type: other.message_type().to_string(),
            }),
        }
    }

    /// Prepare a request against the current space.
    pub fn prepare_request(
        &self,
        spec: PrepareRequestSpec,
    ) -> Result<PreparedRequest, AgentFirstUiError> {
        let space = self.space.as_ref().ok_or(AgentFirstUiError::MissingSpace)?;
        prepare_request(space, spec)
    }
}

/// Prepare a request against an arbitrary space (without a store).
pub fn prepare_request_against_space(
    space: &Space,
    spec: PrepareRequestSpec,
) -> Result<PreparedRequest, AgentFirstUiError> {
    prepare_request(space, spec)
}
