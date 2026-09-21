// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! In-memory bookkeeping of running work: each unit of work owns an event broadcast, a
//! channel for user answers, and a cancellation token.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use crate::protocol::StreamedEvent;

const EVENT_CAPACITY: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkId(String);

impl WorkId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7().simple().to_string())
    }

    pub fn from_string(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Answers delivered by the User Assistant for one information request.
#[derive(Debug, Clone)]
pub struct AnswerDelivery {
    pub request_id: String,
    pub answers: Vec<String>,
}

/// The live handles of one running unit of work.
#[derive(Debug, Clone)]
pub struct WorkHandle {
    pub work_id: WorkId,
    events: broadcast::Sender<StreamedEvent>,
    answers: mpsc::Sender<AnswerDelivery>,
    pub cancellation: CancellationToken,
    sequence: Arc<AtomicU64>,
    last_information_request: Arc<Mutex<Option<StreamedEvent>>>,
}

impl WorkHandle {
    pub fn new(work_id: WorkId) -> (Self, mpsc::Receiver<AnswerDelivery>) {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (answers, answer_rx) = mpsc::channel(8);
        let handle = Self {
            work_id,
            events,
            answers,
            cancellation: CancellationToken::new(),
            sequence: Arc::new(AtomicU64::new(0)),
            last_information_request: Arc::new(Mutex::new(None)),
        };
        (handle, answer_rx)
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamedEvent> {
        self.events.subscribe()
    }

    /// Publishes an event to every subscriber, remembering the latest information
    /// request so a reconnecting client can be told what is still pending.
    pub async fn publish(&self, event: StreamedEvent) {
        if matches!(event, StreamedEvent::InformationRequested { .. }) {
            *self.last_information_request.lock().await = Some(event.clone());
        } else if event.is_terminal() {
            *self.last_information_request.lock().await = None;
        }
        let _ = self.events.send(event);
    }

    pub async fn pending_information_request(&self) -> Option<StreamedEvent> {
        self.last_information_request.lock().await.clone()
    }

    pub async fn clear_pending_information_request(&self) {
        *self.last_information_request.lock().await = None;
    }

    pub async fn deliver_answers(&self, delivery: AnswerDelivery) -> bool {
        self.answers.send(delivery).await.is_ok()
    }
}

/// Registry of the work currently running in this process.
#[derive(Debug, Default)]
pub struct WorkRegistry {
    handles: Mutex<HashMap<String, WorkHandle>>,
}

impl WorkRegistry {
    pub async fn insert(&self, handle: WorkHandle) {
        self.handles
            .lock()
            .await
            .insert(handle.work_id.as_str().to_string(), handle);
    }

    pub async fn get(&self, work_id: &str) -> Option<WorkHandle> {
        self.handles.lock().await.get(work_id).cloned()
    }

    pub async fn remove(&self, work_id: &str) -> Option<WorkHandle> {
        self.handles.lock().await.remove(work_id)
    }

    pub async fn running_ids(&self) -> Vec<String> {
        self.handles.lock().await.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handles_publish_events_and_remember_pending_requests() {
        let (handle, mut answers) = WorkHandle::new(WorkId::generate());
        let mut subscriber = handle.subscribe();
        let request = StreamedEvent::InformationRequested {
            work_id: handle.work_id.to_string(),
            seq: handle.next_sequence(),
            request_id: "r1".into(),
            questions: vec!["q".into()],
            context: String::new(),
        };
        handle.publish(request.clone()).await;
        assert_eq!(subscriber.recv().await.unwrap(), request);
        assert_eq!(handle.pending_information_request().await, Some(request));
        assert!(
            handle
                .deliver_answers(AnswerDelivery {
                    request_id: "r1".into(),
                    answers: vec!["a".into()]
                })
                .await
        );
        assert_eq!(answers.recv().await.unwrap().request_id, "r1");
        handle
            .publish(StreamedEvent::Completed {
                work_id: handle.work_id.to_string(),
                seq: handle.next_sequence(),
                summary: "s".into(),
                artifacts: vec![],
                plan: None,
            })
            .await;
        assert!(handle.pending_information_request().await.is_none());
        assert_eq!(handle.next_sequence(), 3);
    }

    #[tokio::test]
    async fn registry_stores_and_removes_handles() {
        let registry = WorkRegistry::default();
        let (handle, _rx) = WorkHandle::new(WorkId::from_string("w1"));
        registry.insert(handle).await;
        assert!(registry.get("w1").await.is_some());
        assert_eq!(registry.running_ids().await, vec!["w1".to_string()]);
        assert!(registry.remove("w1").await.is_some());
        assert!(registry.get("w1").await.is_none());
    }
}
