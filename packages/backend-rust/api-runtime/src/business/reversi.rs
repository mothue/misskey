/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete invitation reads over native Redis and the shared UserLite batch.
use super::{Error, notes};
use crate::{Runtime, redis::InvitationReadFailure};
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use std::{fmt, future::Future};

const INVITATION_TIMEOUT_MS: i64 = 20_000;

#[derive(Debug)]
pub(crate) enum ReversiFailure {
    InvalidPrincipal,
    ClockRange,
    Redis { source: InvitationReadFailure },
    Pack { source: Box<Error> },
}
impl ReversiFailure {
    pub(super) fn report(&self) {
        let (stage, kind) = match self {
            Self::InvalidPrincipal => ("principal", "invalid_principal"),
            Self::ClockRange => ("window", "clock_range"),
            Self::Redis { source } => ("invitations", source.safe_kind()),
            Self::Pack { .. } => ("pack_ids", "user_lite"),
        };
        tracing::error!(
            operation = "reversi.invitations",
            stage,
            error.kind = kind,
            "endpoint failed"
        );
    }
}
impl fmt::Display for ReversiFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native Reversi invitation read failed")
    }
}
impl std::error::Error for ReversiFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Redis { source } => Some(source),
            // Existing business Error is retained verbatim and delegated to
            // its response boundary; it has no std::error::Error impl.
            Self::Pack { .. } | Self::InvalidPrincipal | Self::ClockRange => None,
        }
    }
}

trait InvitationStore {
    fn members<'a>(
        &'a self,
        viewer_id: &'a str,
        minimum_ms: i64,
    ) -> impl Future<Output = Result<Vec<String>, InvitationReadFailure>> + Send + 'a;
    fn pack<'a>(
        &'a self,
        ids: &'a [String],
        auth: &'a Context,
    ) -> impl Future<Output = Result<Vec<Value>, Error>> + Send + 'a;
}
struct RuntimeStore<'a>(&'a Runtime);
impl InvitationStore for RuntimeStore<'_> {
    fn members<'a>(
        &'a self,
        viewer_id: &'a str,
        minimum_ms: i64,
    ) -> impl Future<Output = Result<Vec<String>, InvitationReadFailure>> + Send + 'a {
        self.0.redis.reversi_invitations(viewer_id, minimum_ms)
    }
    fn pack<'a>(
        &'a self,
        ids: &'a [String],
        auth: &'a Context,
    ) -> impl Future<Output = Result<Vec<Value>, Error>> + Send + 'a {
        notes::pack_id_user_lites(self.0, ids, auth)
    }
}

async fn run(
    store: &(impl InvitationStore + Sync),
    auth: &Context,
    clock: impl FnOnce() -> i64,
) -> Result<Value, ReversiFailure> {
    let viewer_id = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(ReversiFailure::InvalidPrincipal)?;
    // Actual getInvitations reads Date.now once. UserLite online status keeps
    // its separate existing clock, without changing this score boundary.
    let minimum_ms = clock()
        .checked_sub(INVITATION_TIMEOUT_MS)
        .ok_or(ReversiFailure::ClockRange)?;
    let ids = store
        .members(viewer_id, minimum_ms)
        .await
        .map_err(|source| ReversiFailure::Redis { source })?;
    let users = store
        .pack(&ids, auth)
        .await
        .map_err(|source| ReversiFailure::Pack {
            source: Box::new(source),
        })?;
    Ok(Value::Array(users))
}
pub(super) async fn invitations(runtime: &Runtime, auth: &Context) -> Result<Value, Error> {
    run(&RuntimeStore(runtime), auth, crate::now_ms)
        .await
        .map_err(Error::Reversi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    struct Store<'a> {
        source: &'a Json,
        auth: &'a Context,
        calls: Mutex<Vec<&'static str>>,
    }
    impl InvitationStore for Store<'_> {
        async fn members(
            &self,
            viewer_id: &str,
            minimum_ms: i64,
        ) -> Result<Vec<String>, InvitationReadFailure> {
            self.calls.lock().unwrap().push("members");
            assert_eq!(viewer_id, self.source["viewer"]["id"].as_str().unwrap());
            assert_eq!(minimum_ms, self.source["minimum"].as_i64().unwrap());
            Ok(serde_json::from_value(self.source["decodedMembers"].clone()).unwrap())
        }
        async fn pack(&self, ids: &[String], auth: &Context) -> Result<Vec<Value>, Error> {
            self.calls.lock().unwrap().push("pack");
            assert!(
                std::ptr::eq(auth, self.auth),
                "source forwards the actual viewer object"
            );
            let expected: Vec<String> =
                serde_json::from_value(self.source["decodedMembers"].clone()).unwrap();
            assert_eq!(ids, expected.as_slice());
            // A controlled pack result proves callback orchestration only.
            // Real UserLite DTO/privacy/order is checked by public HTTP/E2E.
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn one_clock_window_and_actual_viewer_forwarding_match_extracted_source() {
        let source: Json = serde_json::from_str(include_str!(
            "fixtures/reversi-invitations-source-oracle.json"
        ))
        .unwrap();
        for case in source["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case.get("controlledError").is_none())
        {
            let auth = Context {
                user: Some(case["viewer"].clone()),
                token: None,
                access_token_reduced: false,
            };
            let store = Store {
                source: case,
                auth: &auth,
                calls: Mutex::new(Vec::new()),
            };
            let clock_reads = AtomicUsize::new(0);
            let result = run(&store, &auth, || {
                clock_reads.fetch_add(1, Ordering::Relaxed);
                case["fixedNow"].as_i64().unwrap()
            })
            .await
            .unwrap();
            assert!(matches!(result, Value::Array(_)));
            assert_eq!(
                clock_reads.load(Ordering::Relaxed),
                usize::try_from(case["clockReads"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(*store.calls.lock().unwrap(), ["members", "pack"]);
        }
    }
}
