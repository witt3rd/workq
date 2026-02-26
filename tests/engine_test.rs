//! Integration tests for the work engine.

use serde_json::json;
use workq::engine::{Engine, SubmitResult};
use workq::model::*;

fn test_engine() -> Engine {
    Engine::in_memory().expect("failed to create in-memory engine")
}

// ---------------------------------------------------------------------------
// Basic lifecycle: submit → claim → start → complete
// ---------------------------------------------------------------------------

#[test]
fn submit_creates_queued_work_item() {
    let mut engine = test_engine();

    let result = engine
        .submit(
            NewWorkItem::new("test-work", "unit-test")
                .params(json!({"key": "value"}))
                .priority(5),
        )
        .unwrap();

    match result {
        SubmitResult::Created(item) => {
            assert_eq!(item.work_type, "test-work");
            assert_eq!(item.state, State::Queued);
            assert_eq!(item.priority, 5);
            assert_eq!(item.attempts, 0);
        }
        SubmitResult::Merged { .. } => panic!("expected Created, got Merged"),
    }
}

#[test]
fn full_lifecycle_submit_claim_start_complete() {
    let mut engine = test_engine();

    // Submit
    let item = match engine
        .submit(NewWorkItem::new("test-work", "unit-test"))
        .unwrap()
    {
        SubmitResult::Created(item) => item,
        _ => panic!("expected Created"),
    };
    let id = item.id;

    // Claim
    let claimed = engine
        .claim("worker-1")
        .unwrap()
        .expect("should claim work");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.state, State::Claimed);

    // Start
    engine.start(id, "worker-1").unwrap();
    let running = engine.get(id).unwrap();
    assert_eq!(running.state, State::Running);
    assert_eq!(running.attempts, 1);

    // Complete
    engine
        .complete(
            id,
            Outcome {
                success: true,
                data: Some(json!({"result": "done"})),
                error: None,
                duration_ms: 150,
            },
        )
        .unwrap();

    let completed = engine.get(id).unwrap();
    assert_eq!(completed.state, State::Completed);
    assert!(completed.completed_at.is_some());
}

#[test]
fn claim_returns_none_when_queue_empty() {
    let mut engine = test_engine();
    assert!(engine.claim("worker-1").unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Dedup
// ---------------------------------------------------------------------------

#[test]
fn structural_dedup_merges_duplicate_work() {
    let mut engine = test_engine();

    // First submission — queued
    let first = match engine
        .submit(
            NewWorkItem::new("project-check", "heartbeat")
                .dedup_key("project=garden")
                .priority(1),
        )
        .unwrap()
    {
        SubmitResult::Created(item) => item,
        _ => panic!("first submit should create"),
    };

    // Second submission with same dedup key — should merge
    let second = engine
        .submit(
            NewWorkItem::new("project-check", "initiative")
                .dedup_key("project=garden")
                .priority(5),
        )
        .unwrap();

    match second {
        SubmitResult::Merged { canonical_id, .. } => {
            assert_eq!(canonical_id, first.id);
        }
        SubmitResult::Created(_) => panic!("expected Merged, got Created"),
    }

    // Only one queued item
    let queued = engine.list_by_state(State::Queued).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, first.id);
}

#[test]
fn different_dedup_keys_are_not_merged() {
    let mut engine = test_engine();

    engine
        .submit(NewWorkItem::new("project-check", "heartbeat").dedup_key("project=garden"))
        .unwrap();

    let second = engine
        .submit(NewWorkItem::new("project-check", "heartbeat").dedup_key("project=kitchen"))
        .unwrap();

    assert!(matches!(second, SubmitResult::Created(_)));

    let queued = engine.list_by_state(State::Queued).unwrap();
    assert_eq!(queued.len(), 2);
}

#[test]
fn no_dedup_key_means_no_dedup() {
    let mut engine = test_engine();

    engine
        .submit(NewWorkItem::new("fire-and-forget", "test"))
        .unwrap();

    let second = engine
        .submit(NewWorkItem::new("fire-and-forget", "test"))
        .unwrap();

    assert!(matches!(second, SubmitResult::Created(_)));
}

// ---------------------------------------------------------------------------
// Failure and retry
// ---------------------------------------------------------------------------

#[test]
fn retryable_failure_requeues() {
    let mut engine = test_engine();

    let item = match engine
        .submit(NewWorkItem::new("flaky-work", "test").max_attempts(3))
        .unwrap()
    {
        SubmitResult::Created(item) => item,
        _ => panic!("expected Created"),
    };
    let id = item.id;

    // Claim, start, fail (retryable)
    engine.claim("w1").unwrap();
    engine.start(id, "w1").unwrap();
    engine.fail(id, "transient error", true).unwrap();

    // Should be back in queue
    let item = engine.get(id).unwrap();
    assert_eq!(item.state, State::Queued);

    // Can claim again
    let reclaimed = engine.claim("w2").unwrap().expect("should reclaim");
    assert_eq!(reclaimed.id, id);
}

#[test]
fn non_retryable_failure_goes_dead() {
    let mut engine = test_engine();

    let item = match engine.submit(NewWorkItem::new("bad-work", "test")).unwrap() {
        SubmitResult::Created(item) => item,
        _ => panic!("expected Created"),
    };
    let id = item.id;

    engine.claim("w1").unwrap();
    engine.start(id, "w1").unwrap();
    engine.fail(id, "permanent error", false).unwrap();

    let item = engine.get(id).unwrap();
    assert_eq!(item.state, State::Dead);
}

#[test]
fn exhausted_retries_goes_dead() {
    let mut engine = test_engine();

    let item = match engine
        .submit(NewWorkItem::new("flaky-work", "test").max_attempts(2))
        .unwrap()
    {
        SubmitResult::Created(item) => item,
        _ => panic!("expected Created"),
    };
    let id = item.id;

    // Attempt 1: fail
    engine.claim("w1").unwrap();
    engine.start(id, "w1").unwrap();
    engine.fail(id, "error 1", true).unwrap();
    assert_eq!(engine.get(id).unwrap().state, State::Queued);

    // Attempt 2: fail — should go dead (2/2 exhausted)
    engine.claim("w2").unwrap();
    engine.start(id, "w2").unwrap();
    engine.fail(id, "error 2", true).unwrap();
    assert_eq!(engine.get(id).unwrap().state, State::Dead);
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

#[test]
fn work_scoped_logs() {
    let mut engine = test_engine();

    let item = match engine
        .submit(NewWorkItem::new("logged-work", "test"))
        .unwrap()
    {
        SubmitResult::Created(item) => item,
        _ => panic!("expected Created"),
    };
    let id = item.id;

    engine.log(id, LogLevel::Info, "starting work").unwrap();
    engine
        .log(id, LogLevel::Debug, "querying database")
        .unwrap();
    engine
        .log(id, LogLevel::Error, "something went wrong")
        .unwrap();

    let logs = engine.get_logs(id).unwrap();
    assert_eq!(logs.len(), 3);
    assert_eq!(logs[0].level, LogLevel::Info);
    assert_eq!(logs[0].message, "starting work");
    assert_eq!(logs[2].level, LogLevel::Error);
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[test]
fn events_are_recorded_with_monotonic_seq() {
    let mut engine = test_engine();

    engine
        .submit(NewWorkItem::new("event-test", "test"))
        .unwrap();

    let events = engine.get_events_since(0).unwrap();

    // Should have at least WorkCreated and WorkQueued
    assert!(events.len() >= 2);

    // Sequence numbers are monotonic
    for window in events.windows(2) {
        assert!(window[1].seq > window[0].seq);
    }
}

// ---------------------------------------------------------------------------
// State transition validation
// ---------------------------------------------------------------------------

#[test]
fn invalid_state_transition_errors() {
    let mut engine = test_engine();

    let item = match engine
        .submit(NewWorkItem::new("transition-test", "test"))
        .unwrap()
    {
        SubmitResult::Created(item) => item,
        _ => panic!("expected Created"),
    };

    // Try to complete a queued item (should fail — must go through claimed → running first)
    let result = engine.complete(
        item.id,
        Outcome {
            success: true,
            data: None,
            error: None,
            duration_ms: 0,
        },
    );

    assert!(result.is_err());
}
