//! Session-scoped blob admission, bounded retention and release.
use super::UiBlobRef;
use crate::reserved::SESSION_BLOB_PATH_PREFIX;
use crate::serving::UiBlobPolicy;
use crate::{Error, Result};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use getrandom::fill;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex as StdMutex;

const DEFAULT_RUNTIME_BLOB_CAPACITY: usize = 16;
const RUNTIME_BLOB_TOTAL_MULTIPLIER: u64 = 4;
const BLOB_ID_BYTES: usize = 16;

struct StoredBlob {
    media_type: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct StoredBlobs {
    values: HashMap<String, StoredBlob>,
    order: VecDeque<String>,
    total_bytes: u64,
}

impl StoredBlobs {
    fn remove(&mut self, id: &str) -> bool {
        let Some(removed) = self.values.remove(id) else {
            return false;
        };
        self.total_bytes = self.total_bytes.saturating_sub(removed.bytes.len() as u64);
        self.order.retain(|candidate| candidate != id);
        true
    }
}

pub(super) struct RuntimeBlobStore {
    policy: UiBlobPolicy,
    blobs: StdMutex<StoredBlobs>,
    max_blobs: usize,
    max_total_bytes: u64,
}

impl RuntimeBlobStore {
    pub(super) fn new(policy: UiBlobPolicy) -> Self {
        Self {
            max_blobs: DEFAULT_RUNTIME_BLOB_CAPACITY,
            max_total_bytes: policy
                .max_bytes()
                .saturating_mul(RUNTIME_BLOB_TOTAL_MULTIPLIER),
            policy,
            blobs: StdMutex::new(StoredBlobs::default()),
        }
    }

    pub(super) fn store(&self, media_type: String, bytes: Vec<u8>) -> Result<UiBlobRef> {
        let size_bytes = bytes.len();
        if !self.policy.admits(&media_type)
            || size_bytes as u64 > self.policy.max_bytes()
            || size_bytes as u64 > self.max_total_bytes
        {
            return Err(Error::SessionRuntimeBlobRejected {
                media_type,
                size_bytes,
                max_bytes: self.policy.max_bytes(),
            });
        }
        let id = random_blob_id()?;
        let mut blobs = self.blobs.lock().map_err(|_| Error::SessionRuntimeBusy)?;
        // A collision is vanishingly unlikely, but replacement would corrupt
        // a reference already handed to the page. Treat it as entropy failure.
        if blobs.values.contains_key(&id) {
            return Err(Error::Random(std::io::Error::other(
                "generated a duplicate UI blob identifier",
            )));
        }
        while blobs.values.len() >= self.max_blobs
            || blobs.total_bytes.saturating_add(size_bytes as u64) > self.max_total_bytes
        {
            let Some(oldest) = blobs.order.pop_front() else {
                break;
            };
            blobs.remove(&oldest);
        }
        blobs.total_bytes = blobs.total_bytes.saturating_add(size_bytes as u64);
        blobs.order.push_back(id.clone());
        blobs.values.insert(
            id.clone(),
            StoredBlob {
                media_type: media_type.clone(),
                bytes,
            },
        );
        Ok(UiBlobRef {
            blob_url: format!("{SESSION_BLOB_PATH_PREFIX}{id}"),
            media_type,
            size_bytes,
        })
    }

    pub(super) fn response(&self, id: &str) -> Response {
        let Some((media_type, bytes)) = self.blobs.lock().ok().and_then(|blobs| {
            blobs
                .values
                .get(id)
                .map(|blob| (blob.media_type.clone(), blob.bytes.clone()))
        }) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        self.policy.respond(&media_type, bytes)
    }

    pub(super) fn release_url(&self, blob_url: &str) -> bool {
        let Some(id) = blob_url.strip_prefix(SESSION_BLOB_PATH_PREFIX) else {
            return false;
        };
        if id.is_empty() || id.contains('/') {
            return false;
        }
        let Ok(mut blobs) = self.blobs.lock() else {
            return false;
        };
        blobs.remove(id)
    }
}

fn random_blob_id() -> Result<String> {
    let mut bytes = [0_u8; BLOB_ID_BYTES];
    fill(&mut bytes).map_err(|error| {
        Error::Random(std::io::Error::other(format!(
            "could not generate UI blob identifier: {error}"
        )))
    })?;
    Ok(format!("b{}", hex::encode(bytes)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn status(store: &RuntimeBlobStore, reference: &UiBlobRef) -> StatusCode {
        let id = reference
            .blob_url()
            .strip_prefix(SESSION_BLOB_PATH_PREFIX)
            .unwrap();
        store.response(id).status()
    }

    #[test]
    fn releasing_a_blob_restores_budget_without_evicting_its_neighbors() {
        let store = RuntimeBlobStore::new(UiBlobPolicy::new(8, ["image/png"]));
        let blobs: Vec<_> = (0..4)
            .map(|value| store.store("image/png".into(), vec![value; 8]).unwrap())
            .collect();
        assert!(store.release_url(blobs[1].blob_url()));
        assert!(!store.release_url(blobs[1].blob_url()));
        let replacement = store.store("image/png".into(), vec![4; 8]).unwrap();
        assert_eq!(status(&store, &blobs[0]), StatusCode::OK);
        assert_eq!(status(&store, &blobs[1]), StatusCode::NOT_FOUND);
        assert!(store.store("image/png".into(), vec![0; 9]).is_err());
        assert_eq!(status(&store, &blobs[0]), StatusCode::OK);
        let newest = store.store("image/png".into(), vec![5; 8]).unwrap();
        assert_eq!(status(&store, &blobs[0]), StatusCode::NOT_FOUND);
        for reference in [&blobs[2], &blobs[3], &replacement, &newest] {
            assert_eq!(status(&store, reference), StatusCode::OK);
        }
    }
}
