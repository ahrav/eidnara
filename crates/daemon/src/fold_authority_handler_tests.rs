use super::*;

fn native_config() -> DaemonConfig {
    let mut config = default_test_config();
    config.model_chain.clear();
    config
}

fn bind_with(handler: &Handler, project: &Path, channel: u16, config: DaemonConfig) {
    let mut bound = binding(project.to_str().unwrap(), "ses");
    bound.config = config;
    handler.bind_route(test_route(channel), bound);
}

fn stored_authority(store: &MemoryStore) -> Option<bool> {
    store.load("ses").unwrap().meta.eidnara_folds
}

fn assert_native_state(meta: &ModuleMeta, context: &str) {
    assert_eq!(meta.eidnara_folds, Some(false), "{context}");
    assert!(meta.block_identity_by_mid.is_empty(), "{context}");
    assert!(meta.served_output_fingerprint.is_empty(), "{context}");
    assert_eq!(meta.tail_hygiene_baseline, None, "{context}");
    assert_eq!(meta.coverage_ordinal, None, "{context}");
    assert_eq!(meta.folded_history_segment_seq, 0, "{context}");
}

async fn status_summary(handler: &Handler, channel: u16) -> String {
    let status = call_dispatch_request_on_channel(
        handler,
        channel,
        json!({ "method": "session.status", "v": 1, "session_id": "ses" }),
    )
    .await;
    status["summary"].as_str().unwrap().to_string()
}

async fn quiet_transform(handler: &Handler, messages: Vec<IngressMessage>) -> Value {
    call_transform_request(handler, request_with_usage(messages, 1_000, 50_000)).await
}

#[tokio::test(flavor = "current_thread")]
async fn native_authority_skips_every_fold_step_even_with_a_live_chain() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    bind_with(&handler, &project, 7, native_config());
    store.start_statement_reuse_probe();
    let full_loads = Arc::new(Mutex::new(None));
    {
        let store = Arc::clone(&store);
        let full_loads = Arc::clone(&full_loads);
        *handler
            .between_transform_and_prepare
            .lock()
            .expect("interleave hook mutex") = Some(Box::new(move || {
            *full_loads.lock().expect("hook cell") =
                Some(store.cache_state_load_runs(CacheStateSelect::Full));
        }));
    }

    let response = call_transform(&handler, big_messages()).await;

    assert_eq!(response["status"], "ok", "{response}");
    let diagnostics = &response["history_summarizer"];
    assert_eq!(diagnostics["fired"], false);
    assert_eq!(diagnostics["state"], "disabled");
    assert_eq!(diagnostics["no_fire"], "native_authority");
    let at_hook = full_loads.lock().expect("hook cell").expect("the hook ran");
    assert_eq!(
        store.cache_state_load_runs(CacheStateSelect::Full),
        at_hook,
        "preparation loads nothing under native folds"
    );
    let loaded = store.load("ses").unwrap();
    assert_native_state(&loaded.meta, "native first pass");
    assert_eq!(loaded.meta.history_summarizer.last_no_fire, None);
    assert!(loaded.meta.newest_live_block_id.is_some());
    assert_eq!(
        loaded
            .meta
            .last_usage
            .as_ref()
            .map(|usage| usage.current_total_input_tokens),
        Some(45_000)
    );
    let timings = &response["timings"];
    assert_eq!(timings["trigger_tokenized_blocks"], 0, "{timings}");
    assert_eq!(timings["trigger_eval"], 0.0, "{timings}");
    assert_eq!(timings["trigger_boundary_build"], 0.0, "{timings}");

    let again = call_transform(&handler, big_messages()).await;
    assert_eq!(again["history_summarizer"]["no_fire"], "native_authority");
    assert_eq!(
        store.load("ses").unwrap().row_version,
        loaded.row_version,
        "a repeated native pass writes no no-fire"
    );
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
    assert_eq!(producer.connects.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn wrapup_is_refused_under_native_authority() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    assert_eq!(stored_authority(&store), Some(false));
    cache_wrapup_messages(&handler, wrapup_messages(80, 800));

    let response = tool_body(
        handler
            .dispatch_value(
                test_route(7),
                json!({ "method": "session.wrapup", "v": 1, "session_id": "ses" }),
            )
            .await,
    );

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["reason"], "native_authority", "{response}");
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_first_committing_pass_adopts_over_a_state_sync_row() {
    for eidnara in [false, true] {
        let producer = Arc::new(ProducerState::default());
        let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
        let synced = handler
            .dispatch_value(test_route(7), seed_batch(0, 1, 0, 0))
            .await;
        assert!(matches!(synced, PreparedOutcome::Response(_)), "{synced:?}");
        let row = store.load("ses").unwrap();
        assert!(row.row_version.is_some(), "the sync created the row");
        assert_eq!(
            row.meta.eidnara_folds, None,
            "row existence is not adoption"
        );
        let config = if eidnara {
            default_test_config()
        } else {
            native_config()
        };
        bind_with(&handler, &project, 7, config);

        let _ = quiet_transform(&handler, big_messages()).await;

        assert_eq!(stored_authority(&store), Some(eidnara));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_losing_first_adopter_converges_on_the_winner() {
    let session = "authority-race";
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    for (channel, config) in [(7, native_config()), (8, default_test_config())] {
        let mut bound = binding(project.to_str().unwrap(), session);
        bound.config = config;
        handler.bind_route(test_route(channel), bound);
    }
    let rival = Arc::clone(&store);
    crate::transform::install_transform_attempt_hook(session, move || {
        let loaded = rival.load(session).unwrap();
        let mut meta = loaded.meta.clone();
        meta.eidnara_folds = Some(true);
        rival
            .commit(session, loaded.row_version, &loaded.core, &meta)
            .unwrap();
    });
    let mut losing = request(big_messages());
    losing["session_id"] = json!(session);

    let response = call_transform_request_on_channel(&handler, 7, losing).await;

    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        store.load(session).unwrap().meta.eidnara_folds,
        Some(true),
        "the loser adopts nothing"
    );
    let diagnostics = &response["history_summarizer"];
    assert_ne!(diagnostics["no_fire"], "native_authority", "{diagnostics}");
    assert!(
        diagnostics["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("fold authority pending native")),
        "{diagnostics}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_configuration_adopts_nothing() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    let mut unresolved = native_config();
    unresolved.admission = config::ConfigAdmission::Unresolved {
        reason: "compaction.enabled is not a literal boolean".to_string(),
    };
    bind_with(&handler, &project, 7, unresolved);

    let response = call_transform(&handler, big_messages()).await;

    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(stored_authority(&store), None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_quiescent_bind_changes_authority_in_both_directions_through_the_reset() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    assert_eq!(stored_authority(&store), Some(false));
    let epoch = store.load("ses").unwrap().meta.revert_epoch;

    bind_with(&handler, &project, 7, default_test_config());
    let _ = quiet_transform(&handler, big_messages()).await;
    let eidnara = store.load("ses").unwrap();
    assert_eq!(eidnara.meta.eidnara_folds, Some(true));
    assert_eq!(
        eidnara.meta.revert_epoch,
        epoch + 1,
        "the change is the epoch-fenced reset"
    );

    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    let native = store.load("ses").unwrap();
    assert_native_state(&native.meta, "after the change to native");
    assert_eq!(native.meta.revert_epoch, epoch + 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    assert_eq!(stored_authority(&store), Some(false));

    bind_with(&handler, &project, 8, default_test_config());
    let response = call_transform_request_on_channel(&handler, 8, request(big_messages())).await;
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        stored_authority(&store),
        Some(false),
        "the pass runs under the stored authority"
    );
    let diagnostics = &response["history_summarizer"];
    assert_eq!(diagnostics["no_fire"], "native_authority");
    assert!(
        diagnostics["reason"]
            .as_str()
            .unwrap()
            .contains("fold authority pending eidnara: another binding is open"),
        "{diagnostics}"
    );
    let summary = status_summary(&handler, 8).await;
    assert!(
        summary.starts_with("fold authority native; fold authority pending eidnara"),
        "{summary}"
    );

    handler.unbind_route(test_route(7));
    let _ = call_transform_request_on_channel(&handler, 8, request(big_messages())).await;
    assert_eq!(
        stored_authority(&store),
        Some(false),
        "a later pass of the same binding changes nothing"
    );

    bind_with(&handler, &project, 8, default_test_config());
    let _ = call_transform_request_on_channel(&handler, 8, request(big_messages())).await;
    assert_eq!(stored_authority(&store), Some(true));
}

#[tokio::test(flavor = "current_thread")]
async fn a_busy_summarizer_keeps_the_change_pending_across_a_restart() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    let _ = quiet_transform(&handler, big_messages()).await;
    assert_eq!(stored_authority(&store), Some(true));
    let loaded = store.load("ses").unwrap();
    let mut busy = loaded.meta.clone();
    busy.history_summarizer.state = HistorySummarizerPhase::AwaitingProducer;
    store
        .commit("ses", loaded.row_version, &loaded.core, &busy)
        .unwrap();

    handler.bindings.lock().expect("bindings mutex").clear();
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;

    assert_eq!(stored_authority(&store), Some(true));
    assert!(
        status_summary(&handler, 7)
            .await
            .contains("fold authority pending native")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_ordinary_recomp_reset_preserves_the_adopted_authority() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    let loaded = store.load("ses").unwrap();

    store
        .reset_session_for_recomp("ses", loaded.row_version)
        .unwrap();

    assert_eq!(stored_authority(&store), Some(false));
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_rows_adopt_by_their_fold_artifacts() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) = handler_with_store(producer, default_test_config());
    let _ = call_transform(&handler, big_messages()).await;
    wait_for_idle(&store).await;
    let _ = quiet_transform(&handler, big_messages()).await;
    let folded = store.load("ses").unwrap();
    assert!(!folded.meta.block_identity_by_mid.is_empty());
    assert!(
        folded.meta.has_fold_artifacts(),
        "the folded row carries fold coordinates"
    );
    let mut legacy = folded.meta.clone();
    legacy.eidnara_folds = None;
    store
        .commit("ses", folded.row_version, &folded.core, &legacy)
        .unwrap();
    bind_with(&handler, &project, 7, native_config());

    let mut appended = big_messages();
    appended.push(ck("m81", 81, "turn 81"));
    let _ = call_transform(&handler, appended).await;
    assert_eq!(
        stored_authority(&store),
        Some(true),
        "fold artifacts adopt Eidnara"
    );

    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    let plain = store.load("ses").unwrap();
    let mut legacy = plain.meta.clone();
    legacy.eidnara_folds = None;
    store
        .commit("ses", plain.row_version, &plain.core, &legacy)
        .unwrap();
    bind_with(&handler, &project, 7, native_config());
    let mut appended = big_messages();
    appended.push(ck("m81", 81, "turn 81"));
    let _ = call_transform(&handler, appended).await;
    assert_eq!(
        stored_authority(&store),
        Some(false),
        "no artifacts adopt the binding"
    );

    // A legacy row that holds block identities and no fold coordinates.
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    let additive = store.load("ses").unwrap();
    let mut legacy = additive.meta.clone();
    legacy.eidnara_folds = None;
    legacy.block_identity_by_mid = folded.meta.block_identity_by_mid.clone();
    store
        .commit("ses", additive.row_version, &additive.core, &legacy)
        .unwrap();
    bind_with(&handler, &project, 7, native_config());
    let mut appended = big_messages();
    appended.push(ck("m81", 81, "turn 81"));
    let response = call_transform(&handler, appended).await;
    assert_eq!(
        stored_authority(&store),
        Some(false),
        "stored identities alone adopt the binding"
    );
    assert_eq!(
        response["history_summarizer"]["no_fire"], "native_authority",
        "{response}"
    );
    assert_eq!(
        store.load("ses").unwrap().meta.revert_epoch,
        additive.meta.revert_epoch,
        "adoption over the legacy row runs no authority reset"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn session_status_names_the_applied_authority_and_the_user_config_path() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, dir, project) = handler_with_store(producer, default_test_config());
    let mut config = native_config();
    let user_config = dir
        .path()
        .join("config")
        .join("eidnara")
        .join("eidnara.jsonc");
    config.user_config_path = Some(user_config.clone());
    bind_with(&handler, &project, 7, config);
    let _ = call_transform(&handler, big_messages()).await;

    let summary = status_summary(&handler, 7).await;

    assert!(
        summary.starts_with(&format!(
            "fold authority native; user config encoded:{}; session",
            user_config.display()
        )),
        "{summary}"
    );
}

struct Descent {
    handler: Handler,
    store: Arc<MemoryStore>,
    _dir: tempfile::TempDir,
    project: PathBuf,
    request: Value,
    response: Value,
}

impl Descent {
    fn target(&self) -> memory_store::LoadedState {
        self.store.load(DESCENT_TARGET).unwrap()
    }
}

const DESCENT_TARGET: &str = "authority-lineage-target";

async fn descend_from(source_config: DaemonConfig) -> Descent {
    descend_with_head(source_config, lineage_summary_user()).await
}

async fn descend_with_head(source_config: DaemonConfig, head: IngressMessage) -> Descent {
    let target = DESCENT_TARGET;
    let source = "authority-lineage-source";
    let (handler, store, dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let mut source_binding = binding(project.to_str().unwrap(), source);
    source_binding.config = source_config;
    handler.bind_route(test_route(8), source_binding);
    handler.bind_route(test_route(7), binding(project.to_str().unwrap(), target));
    let source_messages = (1..=10)
        .map(|ordinal| {
            ck(
                &format!("prior-{ordinal}"),
                ordinal,
                &format!("turn {ordinal}"),
            )
        })
        .collect::<Vec<_>>();
    let source_request = native_cache_request(source, source_messages, Vec::new());
    let source_response = call_transform_request_on_channel(
        &handler,
        8,
        serde_json::to_value(source_request).unwrap(),
    )
    .await;
    assert_eq!(source_response["status"], "ok", "{source_response}");
    let source_epoch = store.load(source).unwrap().meta.revert_epoch;
    let configure_lineage = |request: &mut TransformRequest, subagent: bool| {
        request.lineage_switched = true;
        request.is_subagent = subagent;
        request.descent_edge_id = 101;
        request.prior_conversation_key = source.to_string();
        request.prior_epoch = source_epoch;
        request.new_epoch = source_epoch.saturating_add(1);
        request.constituents = vec![(
            source.to_string(),
            target.to_string(),
            source_epoch.saturating_add(1),
        )];
        request.compaction_observed = true;
    };
    let messages = vec![
        head,
        wire_with_role("lineage-tail", 2, "assistant", "continued answer"),
    ];

    let mut subagent = native_cache_request(target, messages.clone(), Vec::new());
    configure_lineage(&mut subagent, true);
    let passthrough =
        call_transform_request(&handler, serde_json::to_value(subagent).unwrap()).await;
    assert_eq!(passthrough["status"], "ok", "{passthrough}");
    assert_eq!(
        store.load(target).unwrap().meta.eidnara_folds,
        None,
        "a protocol passthrough adopts nothing"
    );

    let mut descent = native_cache_request(target, messages, Vec::new());
    configure_lineage(&mut descent, false);
    let request = serde_json::to_value(descent).unwrap();
    let response = call_transform_request(&handler, request.clone()).await;
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(response["lineage_switch_consumed_id"], 101);
    Descent {
        handler,
        store,
        _dir: dir,
        project,
        request,
        response,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_descended_target_carries_the_source_authority() {
    let descent = descend_from(default_test_config()).await;
    let target = descent.target();

    assert_eq!(descent.response["lineage_descent_disposition"], "descended");
    assert_eq!(target.meta.eidnara_folds, Some(true));
    assert!(target.meta.descent_completed);
}

#[tokio::test(flavor = "current_thread")]
async fn a_resent_descent_after_a_change_to_native_is_acknowledged_as_a_replay() {
    let descent = descend_from(default_test_config()).await;
    let descended = descent.target();
    let mut native = binding(descent.project.to_str().unwrap(), DESCENT_TARGET);
    native.config = native_config();
    descent.handler.bind_route(test_route(7), native);

    let resent = call_transform_request(&descent.handler, descent.request.clone()).await;

    assert_eq!(resent["status"], "ok", "{resent}");
    assert_eq!(resent["lineage_switch_consumed_id"], 101);
    assert_eq!(resent["lineage_descent_disposition"], "replay");
    let target = descent.target();
    assert_native_state(&target.meta, "resent descent under native authority");
    assert_eq!(target.meta.revert_epoch, descended.meta.revert_epoch + 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_early_disposition_target_adopts_its_own_binding() {
    let head = wire_with_role("lineage-head", 1, "user", "an ordinary question");
    let descent = descend_with_head(native_config(), head).await;
    let target = descent.target();

    assert_eq!(
        descent.response["lineage_descent_disposition"],
        "not_compaction_shape"
    );
    assert_eq!(target.meta.eidnara_folds, Some(true));
    assert!(!target.meta.descent_completed);
}

#[tokio::test(flavor = "current_thread")]
async fn a_descended_target_whose_binding_disagrees_resets_to_a_first_pass() {
    let descent = descend_from(native_config()).await;
    let target = descent.target();

    assert_eq!(descent.response["lineage_descent_disposition"], "replay");
    assert_eq!(
        target.meta.revert_epoch, 1,
        "one epoch-fenced reset follows the descent"
    );
    assert_eq!(target.meta.eidnara_folds, Some(true));
    assert!(!target.meta.descent_completed);
    assert_eq!(target.meta.ordinal_continuation_base, None);
    assert!(
        target
            .meta
            .last_recut
            .as_deref()
            .is_some_and(|recut| recut.starts_with("fold authority reset to Eidnara")),
        "{:?}",
        target.meta.last_recut
    );
}

async fn native_metadata_bytes(history: u64) -> usize {
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    bind_with(&handler, &project, 7, native_config());
    let full = (1..=history)
        .map(|ordinal| ck(&format!("m{ordinal}"), ordinal, &format!("turn {ordinal}")))
        .collect::<Vec<_>>();
    let response = call_transform(&handler, full).await;
    assert_eq!(response["status"], "ok", "{response}");
    let mut slice = vec![wire_with_role(
        "native-summary",
        1,
        "user",
        "What did we do so far? The conversation so far is summarized here.",
    )];
    slice.extend((1..=40).map(|offset| {
        let ordinal = offset + 1;
        ck(
            &format!("m{}", history + offset),
            ordinal,
            &format!("turn {}", history + offset),
        )
    }));
    let response = call_transform(&handler, slice).await;
    assert_eq!(response["status"], "ok", "{response}");
    let meta = store.load("ses").unwrap().meta;
    assert_native_state(&meta, "native slice");
    serde_json::to_vec(&meta).unwrap().len()
}

#[tokio::test(flavor = "current_thread")]
async fn native_metadata_does_not_grow_with_the_message_count() {
    let small = native_metadata_bytes(1_000).await;
    let large = native_metadata_bytes(20_000).await;

    assert!(
        large.abs_diff(small) * 100 <= small,
        "metadata at 20k messages is {large} bytes against {small} at 1k"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_emergency_rerun_after_publication_keeps_the_change_pending() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    let (handler, store, _dir, project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = big_messages();
    let first = call_transform(&handler, messages.clone()).await;
    assert_eq!(first["history_summarizer"]["fired"], true);
    wait_for_count(&producer.starts, 1).await;
    let epoch = store.load("ses").unwrap().meta.revert_epoch;

    bind_with(&handler, &project, 7, native_config());
    let runner = blocking_unit_tests::JoinedUnitRunner::default();
    let emergency =
        handler.test_client_prepare_request(request_with_usage(messages, 48_000, 50_000));
    let mut blocked =
        Box::pin(handler.handle_transform_with_runner(test_route(7), emergency.clone(), &runner));
    std::future::poll_fn(|cx| {
        assert!(blocked.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    wait_for_count(&runner.completed, 1).await;
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
    let response = tool_body(handler.test_client_apply(&emergency, blocked.await));

    assert!(response["action"].is_string(), "{response}");
    assert!(
        m0_text(&response).contains("autonomous summary"),
        "the rerun refolds with the published summary"
    );
    wait_for_idle(&store).await;
    let loaded = store.load("ses").unwrap();
    assert_eq!(loaded.meta.eidnara_folds, Some(true));
    assert_eq!(loaded.meta.revert_epoch, epoch, "the rerun resets nothing");
    assert!(
        status_summary(&handler, 7)
            .await
            .contains("fold authority pending native"),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_pass_whose_route_is_unbound_while_it_waits_keeps_its_binding_intent() {
    let session = "authority-unbound-wait";
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let mut bound = binding(project.to_str().unwrap(), session);
    bound.config = native_config();
    handler.bind_route(test_route(8), bound);
    let units = Arc::clone(&handler.transform_units)
        .acquire_many_owned(crate::transform_unit::TRANSFORM_UNITS_AT_ONCE as u32)
        .await
        .unwrap();
    let mut request = request(big_messages());
    request["session_id"] = json!(session);
    let mut waiting = Box::pin(call_transform_request_on_channel(&handler, 8, request));
    std::future::poll_fn(|cx| {
        assert!(waiting.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;

    handler.unbind_route(test_route(8));
    drop(units);
    let response = waiting.await;

    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        store.load(session).unwrap().meta.eidnara_folds,
        Some(false),
        "the pass adopts its own binding's authority"
    );
    assert_eq!(
        response["history_summarizer"]["no_fire"], "native_authority",
        "{response}"
    );
    let bindings = handler.bindings.lock().expect("bindings mutex");
    assert!(
        !bindings.by_route[&test_route(7)].first_pass_settled,
        "another session's binding keeps its first pass"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_fold_artifact_written_after_the_plan_replans_the_adoption() {
    let session = "authority-late-artifact";
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let mut bound = binding(project.to_str().unwrap(), session);
    bound.config = native_config();
    handler.bind_route(test_route(9), bound);
    store
        .commit(session, None, &CoreState::empty(), &ModuleMeta::default())
        .unwrap();
    let writer = Arc::clone(&store);
    crate::transform::install_transform_attempt_hook(
        &format!("fold_authority:{session}"),
        move || {
            let loaded = writer.load(session).unwrap();
            let mut meta = loaded.meta.clone();
            meta.history_summarizer.state = HistorySummarizerPhase::AwaitingProducer;
            writer
                .commit(session, loaded.row_version, &loaded.core, &meta)
                .unwrap();
        },
    );
    let mut pass = request(big_messages());
    pass["session_id"] = json!(session);

    let response = call_transform_request_on_channel(&handler, 9, pass).await;

    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        store.load(session).unwrap().meta.eidnara_folds,
        Some(true),
        "the legacy fold artifact decides the adoption"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_sibling_bound_during_the_change_keeps_it_pending() {
    let session = "authority-interleave";
    let (handler, store, _dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let bound = |config: DaemonConfig| {
        let mut bound = binding(project.to_str().unwrap(), session);
        bound.config = config;
        bound
    };
    let pass = || {
        let mut pass = request_with_usage(big_messages(), 1_000, 50_000);
        pass["session_id"] = json!(session);
        pass
    };
    handler.bind_route(test_route(7), bound(native_config()));
    let _ = call_transform_request_on_channel(&handler, 7, pass()).await;
    let native = store.load(session).unwrap();
    assert_eq!(native.meta.eidnara_folds, Some(false));
    handler.bind_route(test_route(7), bound(default_test_config()));
    let bindings = Arc::clone(&handler.bindings);
    let sibling = bound(native_config());
    crate::transform::install_transform_attempt_hook(
        &format!("fold_authority:{session}"),
        move || {
            bindings
                .lock()
                .expect("bindings mutex")
                .insert(test_route(8), sibling.clone());
        },
    );

    let raced = call_transform_request_on_channel(&handler, 7, pass()).await;

    assert_eq!(raced["status"], "ok", "{raced}");
    let kept = store.load(session).unwrap();
    assert_eq!(kept.meta.eidnara_folds, Some(false));
    assert_eq!(kept.meta.revert_epoch, native.meta.revert_epoch);
    let diagnostics = &raced["history_summarizer"];
    assert_eq!(diagnostics["no_fire"], "native_authority", "{diagnostics}");
    assert!(
        diagnostics["reason"].as_str().is_some_and(
            |reason| reason.contains("fold authority pending eidnara: another binding is open")
        ),
        "{diagnostics}"
    );
}

fn restarted(dir: &tempfile::TempDir) -> (Handler, Arc<MemoryStore>) {
    let data_home = dir.path().join("data");
    let store =
        Arc::new(MemoryStore::open(&dev_descriptor_at(data_home.to_str().unwrap())).unwrap());
    let handler = Handler::with_producer_factory_and_config(
        Arc::new(TestProducerFactory {
            state: Arc::new(ProducerState::default()),
        }),
        default_test_config(),
    );
    handler.install_store_for_test(Arc::clone(&store));
    (handler, store)
}

#[tokio::test(flavor = "current_thread")]
async fn authority_changes_survive_restarts_and_serve_first_passes() {
    let (handler, store, dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let _ = quiet_transform(&handler, big_messages()).await;
    let folded = store.load("ses").unwrap();
    assert_eq!(folded.meta.eidnara_folds, Some(true));
    assert!(!folded.meta.block_identity_by_mid.is_empty());
    drop((handler, store));

    let (handler, store) = restarted(&dir);
    bind_with(&handler, &project, 7, native_config());
    let _ = call_transform(&handler, big_messages()).await;
    let native = store.load("ses").unwrap();
    assert_native_state(&native.meta, "after the change to native across a restart");
    assert_eq!(native.meta.revert_epoch, folded.meta.revert_epoch + 1);
    drop((handler, store));

    let (handler, store) = restarted(&dir);
    bind_with(&handler, &project, 7, default_test_config());
    let slice = big_messages_from(40);
    let response = quiet_transform(&handler, slice).await;
    assert_eq!(response["status"], "ok", "{response}");
    let eidnara = store.load("ses").unwrap();
    assert_eq!(eidnara.meta.eidnara_folds, Some(true));
    assert_eq!(eidnara.meta.revert_epoch, native.meta.revert_epoch + 1);
    assert_eq!(eidnara.meta.coverage_ordinal, None);
}

#[derive(Debug, Clone, Copy)]
enum AuthorityOp {
    Bind { channel: u16, eidnara: bool },
    Unbind { channel: u16 },
    Transform { channel: u16 },
    Status { channel: u16 },
    Restart,
}

#[derive(Default)]
struct AuthorityModel {
    stored: Option<bool>,
    epoch: u64,
    bound: BTreeMap<u16, (bool, bool)>,
}

impl AuthorityModel {
    fn transform(&mut self, channel: u16) {
        let Some(&(intent, settled)) = self.bound.get(&channel) else {
            return;
        };
        let sibling = self.bound.len() > 1;
        match self.stored {
            None => self.stored = Some(intent),
            Some(stored) if stored != intent && !settled && !sibling => {
                self.stored = Some(intent);
                self.epoch += 1;
            }
            Some(_) => {}
        }
        self.bound.insert(channel, (intent, true));
    }

    fn summary(&self, channel: u16) -> Option<String> {
        let &(intent, settled) = self.bound.get(&channel)?;
        let name = fold_authority::authority_name;
        Some(match self.stored {
            None => format!("fold authority unadopted (intent {})", name(intent)),
            Some(stored) if stored != intent => {
                let reason = if !settled && self.bound.len() > 1 {
                    fold_authority::PendingReason::SiblingBound
                } else {
                    fold_authority::PendingReason::LaterBind
                };
                let pending = fold_authority::PendingAuthority {
                    target: intent,
                    reason,
                };
                format!("fold authority {}; {}", name(stored), pending.describe())
            }
            Some(stored) => format!("fold authority {}", name(stored)),
        })
    }
}

fn next_op(state: &mut u64) -> AuthorityOp {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    let channel = 7 + (*state >> 8) as u16 % 2;
    match *state % 16 {
        0..=3 => AuthorityOp::Bind {
            channel,
            eidnara: (*state >> 16).is_multiple_of(2),
        },
        4 => AuthorityOp::Unbind { channel },
        5..=11 => AuthorityOp::Transform { channel },
        12..=14 => AuthorityOp::Status { channel },
        _ => AuthorityOp::Restart,
    }
}

fn aimed_at_a_binding(op: AuthorityOp, model: &AuthorityModel) -> AuthorityOp {
    let channel = match op {
        AuthorityOp::Unbind { channel }
        | AuthorityOp::Transform { channel }
        | AuthorityOp::Status { channel } => channel,
        AuthorityOp::Bind { .. } | AuthorityOp::Restart => return op,
    };
    let Some(&bound) = model
        .bound
        .keys()
        .find(|bound| **bound == channel)
        .or_else(|| model.bound.keys().next())
    else {
        return AuthorityOp::Bind {
            channel,
            eidnara: channel == 7,
        };
    };
    match op {
        AuthorityOp::Unbind { .. } => AuthorityOp::Unbind { channel: bound },
        AuthorityOp::Transform { .. } => AuthorityOp::Transform { channel: bound },
        _ => AuthorityOp::Status { channel: bound },
    }
}

async fn run_authority_history(seed: u64, operations: usize) {
    let (mut handler, mut store, dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    handler.unbind_route(test_route(7));
    let mut model = AuthorityModel::default();
    let mut state = seed.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut trace = Vec::new();
    let mut ordinal = 0u64;
    for _ in 0..operations {
        let op = aimed_at_a_binding(next_op(&mut state), &model);
        trace.push(op);
        let context = || format!("seed {seed}: {trace:?}");
        match op {
            AuthorityOp::Bind { channel, eidnara } => {
                let config = if eidnara {
                    default_test_config()
                } else {
                    native_config()
                };
                bind_with(&handler, &project, channel, config);
                model.bound.insert(channel, (eidnara, false));
            }
            AuthorityOp::Unbind { channel } => {
                handler.unbind_route(test_route(channel));
                model.bound.remove(&channel);
            }
            AuthorityOp::Transform { channel } => {
                ordinal += 1;
                let messages = (1..=ordinal)
                    .map(|n| ck(&format!("m{n}"), n, &format!("turn {n}")))
                    .collect();
                let response = call_transform_request_on_channel(
                    &handler,
                    channel,
                    request_with_usage(messages, 1_000, 50_000),
                )
                .await;
                assert_eq!(response["status"], "ok", "{response} {}", context());
                model.transform(channel);
                let loaded = store.load("ses").unwrap();
                assert_eq!(loaded.meta.eidnara_folds, model.stored, "{}", context());
                assert_eq!(loaded.meta.revert_epoch, model.epoch, "{}", context());
                if loaded.meta.eidnara_folds == Some(false) {
                    assert_native_state(&loaded.meta, &context());
                }
            }
            AuthorityOp::Status { channel } => {
                let expected = model.summary(channel).expect("a bound channel");
                let summary = status_summary(&handler, channel).await;
                assert!(
                    summary.starts_with(&format!("{expected}; ")),
                    "{summary} vs {expected}: {}",
                    context()
                );
            }
            AuthorityOp::Restart => {
                drop((handler, store));
                (handler, store) = restarted(&dir);
                model.bound.clear();
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_operation_histories_follow_the_authority_model() {
    for seed in 0..100 {
        run_authority_history(seed, 32).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_status_names_a_stalled_eidnara_summarizer_in_the_authority_prefix() {
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    let _ = quiet_transform(&handler, big_messages()).await;
    let loaded = store.load("ses").unwrap();
    assert_eq!(loaded.meta.eidnara_folds, Some(true));
    let mut stalled = loaded.meta.clone();
    stalled.history_summarizer.last_no_fire = Some("no_models".to_string());
    store
        .commit("ses", loaded.row_version, &loaded.core, &stalled)
        .unwrap();

    let summary = status_summary(&handler, 7).await;

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../packages/opencode-plugin/src/shared/__fixtures__/fold-authority-status.json"
    ))
    .expect("the status fixture parses");
    let stalled_applied = fixture["stalled_applied"]
        .as_str()
        .expect("stalled_applied");
    assert!(
        summary.starts_with(&format!("fold authority {stalled_applied}; user config ")),
        "{summary}"
    );
}

#[test]
fn status_paths_are_percent_encoded_for_the_summary() {
    assert_eq!(
        status_path(Path::new("/home/a  b/x;y%z/eidnara.jsonc")),
        "encoded:/home/a%20%20b/x%3By%25z/eidnara.jsonc"
    );
    assert_eq!(
        status_path(Path::new("/home/u/.config/eidnara/eidnara.jsonc")),
        "encoded:/home/u/.config/eidnara/eidnara.jsonc"
    );
}
