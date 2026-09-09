-- Baseline schema of the memory store, applied once to a pristine file behind
-- the `fence` and `format_marker` tables the storage crate installs first.
-- The bytes of this file are part of the store identity `storage::open_sqlite`
-- checks on every open; there is no upgrade path, so a schema change before
-- genesis edits this file and discards development databases.
--
-- Triggers below call the scalar functions `note_caller_project`,
-- `facade_authority_domain`, and `facade_authority_route`, which
-- `MemoryStore::open` registers after the baseline is applied; SQLite resolves
-- a trigger's functions when the trigger fires, not when it is created.

CREATE TABLE cache_state (
            session_id   TEXT PRIMARY KEY,
            row_version  INTEGER NOT NULL,
            core_state   TEXT NOT NULL,
            meta         TEXT NOT NULL
        , last_activity_at INTEGER NOT NULL DEFAULT 0);

CREATE TABLE compartments (
            session_id        TEXT NOT NULL,
            sequence          INTEGER NOT NULL,
            start_message     INTEGER NOT NULL,
            end_message       INTEGER NOT NULL,
            start_message_id  TEXT NOT NULL DEFAULT '',
            end_message_id    TEXT NOT NULL DEFAULT '',
            title             TEXT NOT NULL,
            content           TEXT NOT NULL,
            p1                TEXT,
            p2                TEXT,
            p3                TEXT,
            p4                TEXT,
            importance        INTEGER NOT NULL DEFAULT 50,
            episode_type      TEXT,
            legacy            INTEGER NOT NULL DEFAULT 0,
            created_at        INTEGER NOT NULL DEFAULT 0, start_date TEXT, end_date TEXT,
            PRIMARY KEY (session_id, sequence)
        );

CREATE TABLE user_memories (
            id                   INTEGER PRIMARY KEY AUTOINCREMENT,
            content              TEXT NOT NULL,
            status               TEXT NOT NULL DEFAULT 'active',
            promoted_at          INTEGER NOT NULL DEFAULT 0,
            source_candidate_ids TEXT DEFAULT '[]',
            created_at           INTEGER NOT NULL DEFAULT 0,
            updated_at           INTEGER NOT NULL DEFAULT 0
        );

CREATE INDEX idx_user_memories_status
            ON user_memories(status);

CREATE TABLE workspaces (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            name             TEXT NOT NULL UNIQUE,
            created_at       INTEGER NOT NULL DEFAULT 0,
            updated_at       INTEGER NOT NULL DEFAULT 0,
            share_categories TEXT NOT NULL DEFAULT '["CONSTRAINTS"]'
        );

CREATE TABLE workspace_members (
            workspace_id  INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
            project_path  TEXT NOT NULL,
            display_name  TEXT NOT NULL,
            display_path  TEXT NOT NULL,
            added_at      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (workspace_id, project_path)
        );

CREATE UNIQUE INDEX idx_workspace_member_unique
            ON workspace_members(project_path);

CREATE TABLE pending_agent_drops (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            target_id   TEXT NOT NULL,
            queued_at   INTEGER NOT NULL DEFAULT 0, command_id TEXT,
            UNIQUE(session_id, target_id)
        );

CREATE INDEX idx_pending_agent_drops_session
            ON pending_agent_drops(session_id, queued_at, id);

CREATE TABLE pass_trace (
            session_id             TEXT PRIMARY KEY,
            last_received_at_ms    INTEGER NOT NULL,
            last_completed_at_ms   INTEGER NOT NULL,
            last_reject_error      TEXT NULL,
            last_reject_at_ms      INTEGER NULL,
            reject_count           INTEGER NOT NULL DEFAULT 0,
            receive_count          INTEGER NOT NULL DEFAULT 0
        , first_divergence TEXT NULL, last_divergence TEXT NULL, scheduler_history TEXT NOT NULL DEFAULT '[]', scheduler_interesting_history TEXT NOT NULL DEFAULT '[]');

CREATE TABLE chunk_transcripts (
            session_id          TEXT NOT NULL,
            compartment_seq     INTEGER NOT NULL,
            start_ordinal       INTEGER NOT NULL,
            end_ordinal         INTEGER NOT NULL,
            transcript_deflate  BLOB NOT NULL,
            created_at_ms       INTEGER NOT NULL, raw_messages_deflate BLOB NULL,
            PRIMARY KEY (session_id, compartment_seq)
        );

CREATE INDEX idx_chunk_transcripts_session_range
            ON chunk_transcripts(session_id, start_ordinal, end_ordinal, compartment_seq);

CREATE TABLE tags (
            session_id     TEXT NOT NULL,
            tag_number    INTEGER NOT NULL,
            block_id      TEXT NOT NULL,
            kind          TEXT NOT NULL CHECK (kind IN ('message', 'tool_call', 'tool_result')),
            token_count   INTEGER NOT NULL DEFAULT 0,
            created_at_ms INTEGER NOT NULL DEFAULT 0, source_bytes BLOB NOT NULL DEFAULT X'',
            PRIMARY KEY (session_id, tag_number),
            UNIQUE(session_id, block_id)
        );

CREATE TABLE channel1_appends (
            session_id     TEXT NOT NULL,
            block_id       TEXT NOT NULL,
            reminder_text  TEXT NOT NULL,
            fired_at_ms    INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (session_id, block_id)
        );

CREATE INDEX idx_channel1_appends_session
            ON channel1_appends(session_id, fired_at_ms, block_id);

CREATE TABLE reduce_command_ledger (
            session_id   TEXT NOT NULL,
            command_id   TEXT NOT NULL,
            queued_at_ms INTEGER NOT NULL, first_applied_at_ms INTEGER, disposition TEXT
            CHECK (disposition IS NULL OR disposition IN ('no_targets')),
            PRIMARY KEY (session_id, command_id)
        );

CREATE INDEX idx_reduce_command_ledger_session_newest
            ON reduce_command_ledger(session_id, queued_at_ms DESC, command_id DESC);

CREATE TABLE user_hints (
            session_id  TEXT NOT NULL,
            block_id    TEXT NOT NULL,
            hint_text   TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            PRIMARY KEY (session_id, block_id)
        );

CREATE INDEX idx_user_hints_session_created
            ON user_hints(session_id, created_at, block_id);

CREATE TABLE overlay_frontiers (
            session_id        TEXT PRIMARY KEY,
            max_seen_ordinal  INTEGER NOT NULL DEFAULT 0
        );

CREATE TABLE temporal_marks (
            session_id   TEXT NOT NULL,
            block_id     TEXT NOT NULL,
            marker_text  TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (session_id, block_id)
        );

CREATE INDEX idx_temporal_marks_session_created
            ON temporal_marks(session_id, created_at, block_id);

CREATE TABLE wrapup_commands (
            session_id   TEXT NOT NULL,
            command_id   TEXT NOT NULL,
            disposition  TEXT NOT NULL
                CHECK (disposition IN ('completed', 'nothing_to_compact', 'failed')),
            rounds       INTEGER NOT NULL,
            summary      TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (session_id, command_id)
        );

CREATE INDEX idx_wrapup_commands_session_created
            ON wrapup_commands(session_id, created_at, command_id);

CREATE INDEX idx_pending_agent_drops_command
            ON pending_agent_drops(session_id, command_id, id);

CREATE TABLE recomp_commands (
            session_id   TEXT NOT NULL,
            command_id   TEXT NOT NULL,
            disposition  TEXT NOT NULL CHECK (disposition IN ('started', 'already_in_progress', 'nothing_to_do')),
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (session_id, command_id)
        );

CREATE INDEX idx_recomp_commands_session_created
            ON recomp_commands(session_id, created_at, command_id);

CREATE TABLE changefeed (
            feed_seq            INTEGER PRIMARY KEY AUTOINCREMENT,
            domain              TEXT NOT NULL CHECK (domain = 'notes'),
            op                  TEXT NOT NULL CHECK (op IN ('insert', 'update', 'tombstone')),
            module_row_id       INTEGER NOT NULL,
            full_row_snapshot   JSON NOT NULL,
            content_hash        TEXT
        );

CREATE INDEX idx_changefeed_domain_seq
            ON changefeed(domain, feed_seq);

CREATE TABLE authority (
            context_store_uuid TEXT NOT NULL,
            project            TEXT NOT NULL,
            domain             TEXT NOT NULL CHECK (domain IN ('memories', 'notes')),
            state               TEXT NOT NULL CHECK (state IN ('TS', 'PREPARING', 'MODULE', 'DRAINING')),
            generation         INTEGER NOT NULL DEFAULT 0,
            captured_upper_bound INTEGER,
            drain_generation   INTEGER,
            drain_cursor       INTEGER NOT NULL DEFAULT 0,
            step_seed          INTEGER NOT NULL DEFAULT 0,
            step_memories      INTEGER NOT NULL DEFAULT 0,
            step_notes         INTEGER NOT NULL DEFAULT 0,
            step_compartments  INTEGER NOT NULL DEFAULT 0,
            step_reconcile     INTEGER NOT NULL DEFAULT 0,
            step_verify        INTEGER NOT NULL DEFAULT 0,
            step_flip          INTEGER NOT NULL DEFAULT 0,
            coordinator_lease TEXT,
            lease_expires_at  INTEGER,
            checksum_expected TEXT,
            checksum_actual   TEXT,
            checksum_ok       INTEGER, coordinator_token TEXT, note_eval_protocol_epoch INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (context_store_uuid, project, domain)
        );

CREATE INDEX idx_authority_project
            ON authority(context_store_uuid, project, state);

CREATE TABLE notes (
            id                         INTEGER PRIMARY KEY AUTOINCREMENT,
            type                       TEXT NOT NULL DEFAULT 'smart'
                CHECK (type IN ('session', 'smart')),
            project_path               TEXT NOT NULL,
            session_id                 TEXT,
            content                    TEXT NOT NULL,
            status                     TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active', 'pending', 'ready', 'surfacing', 'surfaced', 'dismissed')),
            surface_condition          TEXT,
            ready_at                   INTEGER,
            ready_reason               TEXT,
            manifest_json              TEXT,
            compiled_check             TEXT,
            check_hash                 TEXT,
            check_cron                 TEXT,
            check_failure_count       INTEGER NOT NULL DEFAULT 0,
            check_network_failure_count INTEGER NOT NULL DEFAULT 0,
            check_quarantined_until   INTEGER,
            check_next_due_at         INTEGER,
            check_compiled_at         INTEGER,
            check_false_since_at      INTEGER,
            check_last_liveness_at    INTEGER,
            last_checked_at           INTEGER,
            check_status               TEXT NOT NULL DEFAULT 'uncompiled',
            check_version              INTEGER NOT NULL DEFAULT 0,
            policy_version            INTEGER NOT NULL DEFAULT 1,
            harness                    TEXT NOT NULL DEFAULT 'module',
            anchor_block_id            TEXT,
            anchor_ordinal             INTEGER,
            dismissed_at              INTEGER,
            dismissal_resolution       TEXT,
            status_version             INTEGER NOT NULL DEFAULT 0,
            created_at_ms             INTEGER NOT NULL DEFAULT 0,
            updated_at_ms             INTEGER NOT NULL DEFAULT 0,
            context_store_uuid        TEXT,
            context_row_id            INTEGER, source_revision INTEGER NOT NULL DEFAULT 0, state_version INTEGER NOT NULL DEFAULT 0, compiled_source_revision INTEGER, compiled_project_path TEXT, compiled_provider TEXT, compiled_config TEXT, compiled_at INTEGER, compile_status TEXT
            CHECK (compile_status IN ('compiled', 'plain', 'refused') OR compile_status IS NULL),
            UNIQUE(context_store_uuid, context_row_id)
        );

CREATE INDEX idx_notes_scope_status
            ON notes(project_path, session_id, status, updated_at_ms DESC, id DESC);

CREATE INDEX idx_notes_due
            ON notes(project_path, status, check_next_due_at, id);

CREATE TABLE note_deliveries (
            delivery_id                 TEXT PRIMARY KEY,
            note_id                     INTEGER NOT NULL,
            session_id                  TEXT NOT NULL,
            delivered_pass_fingerprint  TEXT NOT NULL,
            transform_pass_id           TEXT NOT NULL DEFAULT '',
            acked_at                    INTEGER,
            created_at_ms               INTEGER NOT NULL DEFAULT 0, project_path TEXT NOT NULL DEFAULT '', disposition TEXT
            CHECK(disposition IS NULL OR disposition IN ('acked','nacked','superseded')),
            UNIQUE(note_id, session_id, delivered_pass_fingerprint)
        );

CREATE TRIGGER notes_ownership_insert
        BEFORE INSERT ON notes
        WHEN NEW.project_path = '' OR note_caller_project() IS NOT NEW.project_path
        BEGIN
            SELECT RAISE(ABORT, 'note ownership insert is outside the caller project');
        END;

CREATE TRIGGER notes_ownership_update
        BEFORE UPDATE ON notes
        WHEN (NEW.id IS NOT OLD.id OR NEW.type IS NOT OLD.type
              OR NEW.session_id IS NOT OLD.session_id OR NEW.project_path IS NOT OLD.project_path
              OR NEW.context_store_uuid IS NOT OLD.context_store_uuid
              OR NEW.context_row_id IS NOT OLD.context_row_id)
          AND NOT (note_caller_project() IS OLD.project_path
                   OR note_caller_project() IS NEW.project_path)
        BEGIN
            SELECT RAISE(ABORT, 'note ownership update is outside the old or new project');
        END;

CREATE TRIGGER notes_ownership_delete
        BEFORE DELETE ON notes
        WHEN note_caller_project() IS NOT OLD.project_path
        BEGIN
            SELECT RAISE(ABORT, 'note ownership delete is outside the row project');
        END;

CREATE TABLE authority_seed_rows (
            context_store_uuid TEXT NOT NULL,
            project TEXT NOT NULL,
            domain TEXT NOT NULL CHECK(domain = 'notes'),
            source_row_id INTEGER NOT NULL,
            snapshot_json TEXT NOT NULL,
            PRIMARY KEY(context_store_uuid, project, domain, source_row_id)
        );

CREATE INDEX idx_note_deliveries_retry
            ON note_deliveries(project_path, session_id, disposition, created_at_ms, note_id);

CREATE TABLE authority_route_bindings (
            route_project_root TEXT PRIMARY KEY,
            context_store_uuid TEXT NOT NULL,
            project            TEXT NOT NULL
        );

CREATE INDEX idx_authority_route_bindings_authority
            ON authority_route_bindings(context_store_uuid, project);

CREATE TABLE dreamer_receipts (
            project TEXT NOT NULL CHECK (length(project) > 0),
            producer TEXT NOT NULL CHECK (length(producer) BETWEEN 1 AND 256),
            operation_key TEXT NOT NULL CHECK (length(operation_key) BETWEEN 1 AND 256),
            database_incarnation_id TEXT NOT NULL CHECK (length(database_incarnation_id) > 0),
            authority_generation INTEGER NOT NULL CHECK (authority_generation >= 0),
            request_encoding_version INTEGER NOT NULL CHECK (request_encoding_version = 1),
            request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
            ledger_session TEXT NOT NULL CHECK (length(ledger_session) > 0),
            command_id TEXT NOT NULL CHECK (length(command_id) BETWEEN 1 AND 256),
            state TEXT NOT NULL CHECK (state IN ('in_progress', 'complete')),
            generation INTEGER NOT NULL CHECK (generation >= 1),
            terminal_kind TEXT CHECK (terminal_kind IN ('complete', 'failed', 'cancelled', 'unknown')),
            result_json TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY (project, producer, operation_key),
            CHECK (
                (state = 'in_progress' AND result_json IS NULL AND terminal_kind IS NULL)
                OR (state = 'complete' AND result_json IS NOT NULL AND terminal_kind IS NOT NULL)
            )
        );

CREATE TABLE dreamer_attempts (
            project TEXT NOT NULL,
            producer TEXT NOT NULL,
            operation_key TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK (generation >= 1),
            attempt_index INTEGER NOT NULL CHECK (attempt_index >= 0),
            model TEXT NOT NULL CHECK (length(model) > 0),
            prompt_template_version INTEGER NOT NULL CHECK (prompt_template_version >= 1),
            system_prompt_hash TEXT NOT NULL CHECK (length(system_prompt_hash) = 64),
            schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
            child_session TEXT NOT NULL CHECK (length(child_session) > 0),
            dispatched_at_ms INTEGER NOT NULL,
            run_handle TEXT,
            terminal_kind TEXT CHECK (terminal_kind IN ('complete', 'failed', 'cancelled', 'unknown')),
            terminal_at_ms INTEGER,
            session_released_at_ms INTEGER,
            PRIMARY KEY (project, producer, operation_key, generation, attempt_index),
            FOREIGN KEY (project, producer, operation_key)
                REFERENCES dreamer_receipts(project, producer, operation_key),
            CHECK ((terminal_kind IS NULL) = (terminal_at_ms IS NULL))
        );

CREATE INDEX idx_dreamer_attempts_project_dispatched
            ON dreamer_attempts(project, dispatched_at_ms);

CREATE TABLE transform_session_roots (
            session_id  TEXT NOT NULL,
            project_root TEXT NOT NULL,
            observed_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, project_root)
        );

CREATE INDEX idx_transform_session_roots_observed
            ON transform_session_roots(observed_at);

CREATE TRIGGER notes_facade_authority_insert
        BEFORE INSERT ON notes
        WHEN facade_authority_domain() = 'notes'
          AND EXISTS (
              SELECT 1 FROM authority_route_bindings binding
              JOIN authority authority
                ON authority.context_store_uuid = binding.context_store_uuid
               AND authority.project = binding.project
             WHERE binding.route_project_root = facade_authority_route()
               AND authority.domain = 'notes'
               AND authority.project = NEW.project_path
               AND authority.state != 'MODULE'
          )
        BEGIN SELECT RAISE(ABORT, 'authority_draining'); END;

CREATE TRIGGER notes_facade_authority_update
        BEFORE UPDATE ON notes
        WHEN facade_authority_domain() = 'notes'
          AND EXISTS (
              SELECT 1 FROM authority_route_bindings binding
              JOIN authority authority
                ON authority.context_store_uuid = binding.context_store_uuid
               AND authority.project = binding.project
             WHERE binding.route_project_root = facade_authority_route()
               AND authority.domain = 'notes'
               AND authority.project IN (OLD.project_path, NEW.project_path)
               AND authority.state != 'MODULE'
          )
        BEGIN SELECT RAISE(ABORT, 'authority_draining'); END;

CREATE TRIGGER notes_facade_authority_delete
        BEFORE DELETE ON notes
        WHEN facade_authority_domain() = 'notes'
          AND EXISTS (
              SELECT 1 FROM authority_route_bindings binding
              JOIN authority authority
                ON authority.context_store_uuid = binding.context_store_uuid
               AND authority.project = binding.project
             WHERE binding.route_project_root = facade_authority_route()
               AND authority.domain = 'notes'
               AND authority.project = OLD.project_path
               AND authority.state != 'MODULE'
          )
        BEGIN SELECT RAISE(ABORT, 'authority_draining'); END;

CREATE TABLE compartment_events (
            id                    INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id            TEXT NOT NULL,
            compartment_id        INTEGER,
            at_compartment        INTEGER,
            kind                  TEXT NOT NULL,
            fields_json           TEXT NOT NULL DEFAULT '{}',
            created_at             INTEGER NOT NULL DEFAULT 0,
            harness                TEXT NOT NULL DEFAULT 'module'
        );

CREATE INDEX idx_compartment_events_session
            ON compartment_events(session_id, id);

CREATE TABLE primer_candidates (
            id                       INTEGER PRIMARY KEY AUTOINCREMENT,
            project_path             TEXT NOT NULL,
            harness                  TEXT NOT NULL DEFAULT 'module',
            session_id               TEXT NOT NULL,
            question                 TEXT NOT NULL,
            normalized_question      TEXT NOT NULL,
            source_compartment_start INTEGER,
            source_compartment_end   INTEGER,
            source_start_message_id  TEXT NOT NULL DEFAULT '',
            source_end_message_id    TEXT NOT NULL DEFAULT '',
            source_message_time      INTEGER NOT NULL DEFAULT 0,
            created_at               INTEGER NOT NULL DEFAULT 0,
            UNIQUE(project_path, harness, session_id, source_start_message_id, source_end_message_id)
        );

CREATE INDEX idx_primer_candidates_project
            ON primer_candidates(project_path, created_at, id);

CREATE TABLE user_memory_candidates (
            id                       INTEGER PRIMARY KEY AUTOINCREMENT,
            content                  TEXT NOT NULL,
            session_id               TEXT NOT NULL,
            source_compartment_start INTEGER,
            source_compartment_end   INTEGER,
            created_at               INTEGER NOT NULL DEFAULT 0
        );

CREATE INDEX idx_user_memory_candidates_session
            ON user_memory_candidates(session_id, created_at, id);

CREATE TABLE historian_side_channel_outbox (
            session_id          TEXT NOT NULL,
            firing_seq         INTEGER NOT NULL,
            kind               TEXT NOT NULL
                CHECK (kind IN ('event', 'primer', 'user_observation')),
            source_start       INTEGER NOT NULL,
            source_end         INTEGER NOT NULL,
            item_index         INTEGER NOT NULL,
            payload_json       TEXT NOT NULL,
            attempt_count      INTEGER NOT NULL DEFAULT 0,
            next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
            last_attempt_at_ms INTEGER,
            last_error         TEXT,
            delivered_at_ms    INTEGER,
            created_at_ms      INTEGER NOT NULL,
            PRIMARY KEY (session_id, firing_seq, kind, source_start, source_end, item_index)
        );

CREATE INDEX idx_historian_side_channel_outbox_due
            ON historian_side_channel_outbox(
                session_id, kind, delivered_at_ms, next_attempt_at_ms, firing_seq, item_index
            );

CREATE TABLE facade_mutation_ledger (
            identity_scope TEXT NOT NULL,
            tool           TEXT NOT NULL,
            action         TEXT NOT NULL,
            command_id     TEXT NOT NULL,
            response_json  BLOB NOT NULL,
            created_at_ms  INTEGER NOT NULL,
            PRIMARY KEY (identity_scope, tool, action, command_id)
        );

CREATE INDEX idx_facade_mutation_ledger_scope_newest
            ON facade_mutation_ledger(identity_scope, created_at_ms DESC, tool, action, command_id);

CREATE INDEX idx_compartments_session_end_message
            ON compartments(session_id, end_message);

CREATE INDEX idx_notes_project_status_updated
            ON notes(project_path, status, updated_at_ms DESC, id DESC);

CREATE INDEX idx_historian_side_channel_outbox_order
            ON historian_side_channel_outbox(
                session_id, kind, delivered_at_ms,
                firing_seq, source_start, source_end, item_index, next_attempt_at_ms
            );

CREATE TABLE tag_cache_generations (
            session_id TEXT PRIMARY KEY,
            generation INTEGER NOT NULL DEFAULT 0,
            tag_count INTEGER NOT NULL DEFAULT 0,
            max_tag_number INTEGER NOT NULL DEFAULT 0
        );

CREATE TRIGGER tags_cache_generation_insert AFTER INSERT ON tags BEGIN
            INSERT INTO tag_cache_generations(session_id, generation, tag_count, max_tag_number)
            VALUES (NEW.session_id, 1, 1, NEW.tag_number)
            ON CONFLICT(session_id) DO UPDATE SET
                generation = generation + 1,
                tag_count = tag_count + 1,
                max_tag_number = MAX(max_tag_number, NEW.tag_number);
        END;

CREATE TRIGGER tags_cache_generation_delete AFTER DELETE ON tags BEGIN
            INSERT INTO tag_cache_generations(session_id, generation, tag_count, max_tag_number)
            VALUES (
                OLD.session_id,
                1,
                (SELECT COUNT(*) FROM tags WHERE session_id = OLD.session_id),
                (SELECT COALESCE(MAX(tag_number), 0) FROM tags WHERE session_id = OLD.session_id)
            )
            ON CONFLICT(session_id) DO UPDATE SET
                generation = generation + 1,
                tag_count = excluded.tag_count,
                max_tag_number = excluded.max_tag_number;
        END;

CREATE TRIGGER tags_cache_generation_update AFTER UPDATE ON tags BEGIN
            INSERT INTO tag_cache_generations(session_id, generation, tag_count, max_tag_number)
            VALUES (
                OLD.session_id,
                1,
                (SELECT COUNT(*) FROM tags WHERE session_id = OLD.session_id),
                (SELECT COALESCE(MAX(tag_number), 0) FROM tags WHERE session_id = OLD.session_id)
            )
            ON CONFLICT(session_id) DO UPDATE SET
                generation = generation + 1,
                tag_count = excluded.tag_count,
                max_tag_number = excluded.max_tag_number;
            INSERT INTO tag_cache_generations(session_id, generation, tag_count, max_tag_number)
            VALUES (
                NEW.session_id,
                1,
                (SELECT COUNT(*) FROM tags WHERE session_id = NEW.session_id),
                (SELECT COALESCE(MAX(tag_number), 0) FROM tags WHERE session_id = NEW.session_id)
            )
            ON CONFLICT(session_id) DO UPDATE SET
                generation = generation + 1,
                tag_count = excluded.tag_count,
                max_tag_number = excluded.max_tag_number;
        END;

CREATE TABLE project_mural_artifacts (
            project_path TEXT PRIMARY KEY NOT NULL,
            data_url BLOB NOT NULL,
            content_hash TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

CREATE TRIGGER notes_feed_insert AFTER INSERT ON notes BEGIN
            INSERT INTO changefeed(domain, op, module_row_id, full_row_snapshot, content_hash)
            VALUES ('notes', 'insert', NEW.id,
                json_object(
                    'id', NEW.id, 'type', NEW.type, 'project_path', NEW.project_path,
                    'session_id', NEW.session_id, 'content', NEW.content, 'status', NEW.status,
                    'surface_condition', NEW.surface_condition, 'ready_at', NEW.ready_at,
                    'ready_reason', NEW.ready_reason, 'manifest_json', NEW.manifest_json,
                    'compiled_check', NEW.compiled_check, 'check_hash', NEW.check_hash,
                    'check_cron', NEW.check_cron, 'check_failure_count', NEW.check_failure_count,
                    'check_network_failure_count', NEW.check_network_failure_count,
                    'check_quarantined_until', NEW.check_quarantined_until,
                    'check_next_due_at', NEW.check_next_due_at, 'check_compiled_at', NEW.check_compiled_at,
                    'check_false_since_at', NEW.check_false_since_at,
                    'check_last_liveness_at', NEW.check_last_liveness_at,
                    'last_checked_at', NEW.last_checked_at, 'check_status', NEW.check_status,
                    'check_version', NEW.check_version, 'policy_version', NEW.policy_version,
                    'harness', NEW.harness, 'anchor_block_id', NEW.anchor_block_id,
                    'anchor_ordinal', NEW.anchor_ordinal, 'dismissed_at', NEW.dismissed_at,
                    'dismissal_resolution', NEW.dismissal_resolution,
                    'status_version', NEW.status_version, 'created_at_ms', NEW.created_at_ms,
                    'updated_at_ms', NEW.updated_at_ms, 'context_store_uuid', NEW.context_store_uuid,
                    'context_row_id', NEW.context_row_id,
                    'source_revision', NEW.source_revision, 'state_version', NEW.state_version,
                    'compiled_source_revision', NEW.compiled_source_revision,
                    'compiled_project_path', NEW.compiled_project_path,
                    'compiled_provider', NEW.compiled_provider,
                    'compiled_config', NEW.compiled_config,
                    'compiled_at', NEW.compiled_at, 'compile_status', NEW.compile_status), NULL);
        END;

CREATE TRIGGER notes_feed_update AFTER UPDATE ON notes
        WHEN NEW.id IS NOT OLD.id OR NEW.type IS NOT OLD.type
          OR NEW.project_path IS NOT OLD.project_path OR NEW.session_id IS NOT OLD.session_id
          OR NEW.content IS NOT OLD.content OR NEW.status IS NOT OLD.status
          OR NEW.surface_condition IS NOT OLD.surface_condition OR NEW.ready_at IS NOT OLD.ready_at
          OR NEW.ready_reason IS NOT OLD.ready_reason OR NEW.manifest_json IS NOT OLD.manifest_json
          OR NEW.compiled_check IS NOT OLD.compiled_check OR NEW.check_hash IS NOT OLD.check_hash
          OR NEW.check_cron IS NOT OLD.check_cron
          OR NEW.check_failure_count IS NOT OLD.check_failure_count
          OR NEW.check_network_failure_count IS NOT OLD.check_network_failure_count
          OR NEW.check_quarantined_until IS NOT OLD.check_quarantined_until
          OR NEW.check_next_due_at IS NOT OLD.check_next_due_at
          OR NEW.check_compiled_at IS NOT OLD.check_compiled_at
          OR NEW.check_false_since_at IS NOT OLD.check_false_since_at
          OR NEW.check_last_liveness_at IS NOT OLD.check_last_liveness_at
          OR NEW.last_checked_at IS NOT OLD.last_checked_at OR NEW.check_status IS NOT OLD.check_status
          OR NEW.check_version IS NOT OLD.check_version OR NEW.policy_version IS NOT OLD.policy_version
          OR NEW.harness IS NOT OLD.harness OR NEW.anchor_block_id IS NOT OLD.anchor_block_id
          OR NEW.anchor_ordinal IS NOT OLD.anchor_ordinal OR NEW.dismissed_at IS NOT OLD.dismissed_at
          OR NEW.dismissal_resolution IS NOT OLD.dismissal_resolution
          OR NEW.status_version IS NOT OLD.status_version
          OR NEW.created_at_ms IS NOT OLD.created_at_ms OR NEW.updated_at_ms IS NOT OLD.updated_at_ms
          OR NEW.context_store_uuid IS NOT OLD.context_store_uuid
          OR NEW.context_row_id IS NOT OLD.context_row_id
          OR NEW.source_revision IS NOT OLD.source_revision
          OR NEW.state_version IS NOT OLD.state_version
          OR NEW.compiled_source_revision IS NOT OLD.compiled_source_revision
          OR NEW.compiled_project_path IS NOT OLD.compiled_project_path
          OR NEW.compiled_provider IS NOT OLD.compiled_provider
          OR NEW.compiled_config IS NOT OLD.compiled_config
          OR NEW.compiled_at IS NOT OLD.compiled_at
          OR NEW.compile_status IS NOT OLD.compile_status
        BEGIN
            INSERT INTO changefeed(domain, op, module_row_id, full_row_snapshot, content_hash)
            VALUES ('notes', 'update', NEW.id,
                json_object(
                    'id', NEW.id, 'type', NEW.type, 'project_path', NEW.project_path,
                    'session_id', NEW.session_id, 'content', NEW.content, 'status', NEW.status,
                    'surface_condition', NEW.surface_condition, 'ready_at', NEW.ready_at,
                    'ready_reason', NEW.ready_reason, 'manifest_json', NEW.manifest_json,
                    'compiled_check', NEW.compiled_check, 'check_hash', NEW.check_hash,
                    'check_cron', NEW.check_cron, 'check_failure_count', NEW.check_failure_count,
                    'check_network_failure_count', NEW.check_network_failure_count,
                    'check_quarantined_until', NEW.check_quarantined_until,
                    'check_next_due_at', NEW.check_next_due_at, 'check_compiled_at', NEW.check_compiled_at,
                    'check_false_since_at', NEW.check_false_since_at,
                    'check_last_liveness_at', NEW.check_last_liveness_at,
                    'last_checked_at', NEW.last_checked_at, 'check_status', NEW.check_status,
                    'check_version', NEW.check_version, 'policy_version', NEW.policy_version,
                    'harness', NEW.harness, 'anchor_block_id', NEW.anchor_block_id,
                    'anchor_ordinal', NEW.anchor_ordinal, 'dismissed_at', NEW.dismissed_at,
                    'dismissal_resolution', NEW.dismissal_resolution,
                    'status_version', NEW.status_version, 'created_at_ms', NEW.created_at_ms,
                    'updated_at_ms', NEW.updated_at_ms, 'context_store_uuid', NEW.context_store_uuid,
                    'context_row_id', NEW.context_row_id,
                    'source_revision', NEW.source_revision, 'state_version', NEW.state_version,
                    'compiled_source_revision', NEW.compiled_source_revision,
                    'compiled_project_path', NEW.compiled_project_path,
                    'compiled_provider', NEW.compiled_provider,
                    'compiled_config', NEW.compiled_config,
                    'compiled_at', NEW.compiled_at, 'compile_status', NEW.compile_status), NULL);
        END;

CREATE TRIGGER notes_feed_delete AFTER DELETE ON notes BEGIN
            INSERT INTO changefeed(domain, op, module_row_id, full_row_snapshot, content_hash)
            VALUES ('notes', 'tombstone', OLD.id,
                json_object(
                    'id', OLD.id, 'type', OLD.type, 'project_path', OLD.project_path,
                    'session_id', OLD.session_id, 'content', OLD.content, 'status', OLD.status,
                    'surface_condition', OLD.surface_condition, 'ready_at', OLD.ready_at,
                    'ready_reason', OLD.ready_reason, 'manifest_json', OLD.manifest_json,
                    'compiled_check', OLD.compiled_check, 'check_hash', OLD.check_hash,
                    'check_cron', OLD.check_cron, 'check_failure_count', OLD.check_failure_count,
                    'check_network_failure_count', OLD.check_network_failure_count,
                    'check_quarantined_until', OLD.check_quarantined_until,
                    'check_next_due_at', OLD.check_next_due_at, 'check_compiled_at', OLD.check_compiled_at,
                    'check_false_since_at', OLD.check_false_since_at,
                    'check_last_liveness_at', OLD.check_last_liveness_at,
                    'last_checked_at', OLD.last_checked_at, 'check_status', OLD.check_status,
                    'check_version', OLD.check_version, 'policy_version', OLD.policy_version,
                    'harness', OLD.harness, 'anchor_block_id', OLD.anchor_block_id,
                    'anchor_ordinal', OLD.anchor_ordinal, 'dismissed_at', OLD.dismissed_at,
                    'dismissal_resolution', OLD.dismissal_resolution,
                    'status_version', OLD.status_version, 'created_at_ms', OLD.created_at_ms,
                    'updated_at_ms', OLD.updated_at_ms, 'context_store_uuid', OLD.context_store_uuid,
                    'context_row_id', OLD.context_row_id,
                    'source_revision', OLD.source_revision, 'state_version', OLD.state_version,
                    'compiled_source_revision', OLD.compiled_source_revision,
                    'compiled_project_path', OLD.compiled_project_path,
                    'compiled_provider', OLD.compiled_provider,
                    'compiled_config', OLD.compiled_config,
                    'compiled_at', OLD.compiled_at, 'compile_status', OLD.compile_status), NULL);
        END;

CREATE TABLE note_eval_claims (
            claim_id TEXT PRIMARY KEY,
            project TEXT NOT NULL,
            note_id INTEGER NOT NULL,
            phase TEXT NOT NULL CHECK (phase IN ('compile', 'due', 'liveness', 'fallback')),
            acquisition_id TEXT NOT NULL,
            evaluator_instance TEXT NOT NULL,
            evaluator_slot INTEGER NOT NULL,
            registration_generation INTEGER NOT NULL,
            source_revision INTEGER NOT NULL,
            state_version INTEGER NOT NULL,
            policy_version INTEGER NOT NULL,
            protocol_epoch INTEGER NOT NULL,
            authority_generation INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            completion_id TEXT,
            terminal_kind TEXT,
            terminal_response TEXT,
            terminal_at_ms INTEGER,
            UNIQUE (project, acquisition_id)
        );

CREATE UNIQUE INDEX idx_note_eval_claims_active_note
            ON note_eval_claims(project, note_id) WHERE terminal_kind IS NULL;

CREATE UNIQUE INDEX idx_note_eval_claims_active_slot
            ON note_eval_claims(project, evaluator_instance, evaluator_slot)
            WHERE terminal_kind IS NULL;

CREATE TABLE note_eval_acquisitions (
            project TEXT NOT NULL,
            acquisition_id TEXT NOT NULL,
            decision TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            PRIMARY KEY (project, acquisition_id)
        );

CREATE INDEX idx_primer_candidates_session
            ON primer_candidates(session_id, id);

CREATE TABLE claim_intents (
            producer TEXT NOT NULL CHECK (length(producer) BETWEEN 1 AND 256),
            operation_key TEXT NOT NULL CHECK (length(operation_key) BETWEEN 1 AND 256),
            database_incarnation_id TEXT NOT NULL CHECK (length(database_incarnation_id) = 32),
            format_epoch INTEGER NOT NULL CHECK (format_epoch > 0),
            authority_project TEXT NOT NULL CHECK (length(authority_project) > 0),
            authority_generation INTEGER NOT NULL CHECK (authority_generation >= 0),
            request_encoding_version INTEGER NOT NULL CHECK (request_encoding_version = 1),
            request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
            state TEXT NOT NULL CHECK (state IN (
                'staged', 'context-committed', 'acknowledged', 'terminal-rejected'
            )),
            result_json TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY (producer, operation_key),
            CHECK (
                (state = 'staged' AND result_json IS NULL)
                OR (state <> 'staged' AND result_json IS NOT NULL)
            )
        );

CREATE INDEX idx_claim_intents_unresolved
            ON claim_intents(state, created_at_ms, producer, operation_key);

CREATE TABLE claim_intent_controls (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            database_incarnation_id TEXT NOT NULL
                CHECK (length(database_incarnation_id) = 32),
            authority_generation INTEGER NOT NULL CHECK (authority_generation >= 0),
            transition_state TEXT NOT NULL CHECK (transition_state IN (
                'accepting', 'draining', 'resetting'
            )),
            updated_at_ms INTEGER NOT NULL
        );

CREATE TABLE claim_mirror_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mirror_version INTEGER NOT NULL CHECK (mirror_version = 1),
            vector_version INTEGER NOT NULL CHECK (vector_version = 1),
            database_incarnation_id TEXT NOT NULL
                CHECK (length(database_incarnation_id) = 32),
            workspace_epoch TEXT NOT NULL CHECK (length(workspace_epoch) > 0),
            updated_at_ms INTEGER NOT NULL
        );

CREATE TABLE claim_mirror_projects (
            database_incarnation_id TEXT NOT NULL
                CHECK (length(database_incarnation_id) = 32),
            project_id INTEGER NOT NULL CHECK (project_id > 0),
            project_generation INTEGER NOT NULL CHECK (project_generation >= 0),
            policy_generation INTEGER NOT NULL CHECK (policy_generation >= 0),
            acked_effect_id INTEGER NOT NULL CHECK (acked_effect_id >= 0),
            PRIMARY KEY (database_incarnation_id, project_id)
        ) WITHOUT ROWID;

CREATE TABLE claim_mirror_claims (
            database_incarnation_id TEXT NOT NULL
                CHECK (length(database_incarnation_id) = 32),
            public_claim_id TEXT NOT NULL CHECK (length(public_claim_id) = 36),
            project_id INTEGER NOT NULL CHECK (project_id > 0),
            revision_locator TEXT NOT NULL CHECK (length(revision_locator) > 0),
            revision INTEGER NOT NULL CHECK (revision > 0),
            content TEXT NOT NULL,
            content_digest TEXT NOT NULL CHECK (length(content_digest) = 64),
            attributes_json TEXT NOT NULL CHECK (json_valid(attributes_json)),
            lifecycle_state TEXT NOT NULL CHECK (
                lifecycle_state IN ('active', 'archived', 'retired')
            ),
            applicability_json TEXT NOT NULL CHECK (json_valid(applicability_json)),
            policy_json TEXT NOT NULL CHECK (json_valid(policy_json)),
            provenance_label TEXT,
            project_generation INTEGER NOT NULL CHECK (project_generation >= 0),
            policy_generation INTEGER NOT NULL CHECK (policy_generation >= 0),
            PRIMARY KEY (database_incarnation_id, public_claim_id),
            UNIQUE (database_incarnation_id, revision_locator),
            FOREIGN KEY (database_incarnation_id, project_id)
                REFERENCES claim_mirror_projects(database_incarnation_id, project_id)
                ON DELETE CASCADE
        ) WITHOUT ROWID;

CREATE INDEX idx_claim_mirror_claims_project
            ON claim_mirror_claims(database_incarnation_id, project_id, public_claim_id);

CREATE TABLE claim_mirror_receipts (
            database_incarnation_id TEXT NOT NULL
                CHECK (length(database_incarnation_id) = 32),
            receipt_id INTEGER NOT NULL CHECK (receipt_id > 0),
            expected_effect_count INTEGER NOT NULL CHECK (expected_effect_count > 0),
            first_effect_id INTEGER NOT NULL CHECK (first_effect_id > 0),
            last_effect_id INTEGER NOT NULL CHECK (last_effect_id >= first_effect_id),
            group_digest TEXT NOT NULL CHECK (length(group_digest) = 64),
            generation_vector_json TEXT NOT NULL CHECK (json_valid(generation_vector_json)),
            applied_at_ms INTEGER NOT NULL,
            PRIMARY KEY (database_incarnation_id, receipt_id)
        ) WITHOUT ROWID;


CREATE TABLE scan_batches (
            scan_batch_id TEXT PRIMARY KEY CHECK (length(scan_batch_id) = 32),
            owner_kind TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );

CREATE TABLE scan_owner_scopes (
            owner_scope_id TEXT PRIMARY KEY CHECK (length(owner_scope_id) = 32),
            scope_kind TEXT NOT NULL CHECK (length(scope_kind) > 0),
            scope_key TEXT NOT NULL CHECK (length(scope_key) = 64),
            UNIQUE (scope_kind, scope_key)
        );

CREATE TABLE scan_domain_owners (
            domain_owner_id TEXT PRIMARY KEY CHECK (length(domain_owner_id) = 32),
            owner_scope_id TEXT NOT NULL
                REFERENCES scan_owner_scopes(owner_scope_id) ON DELETE CASCADE,
            owner_kind TEXT NOT NULL,
            owner_key TEXT NOT NULL CHECK (length(owner_key) = 64),
            UNIQUE (owner_scope_id, owner_kind, owner_key)
        );

CREATE TABLE field_scans (
            scan_id TEXT PRIMARY KEY CHECK (length(scan_id) = 32),
            scan_batch_id TEXT NOT NULL REFERENCES scan_batches(scan_batch_id)
                ON DELETE CASCADE,
            detector_id TEXT NOT NULL,
            detector_revision TEXT NOT NULL,
            semantic_digest TEXT CHECK (semantic_digest IS NULL OR length(semantic_digest) = 64),
            finding_count INTEGER NOT NULL CHECK (finding_count >= 0)
        );

CREATE INDEX idx_field_scans_batch
            ON field_scans(scan_batch_id, scan_id);

CREATE TABLE scan_owner_copies (
            owner_copy_id TEXT PRIMARY KEY CHECK (length(owner_copy_id) = 32),
            scan_id TEXT NOT NULL REFERENCES field_scans(scan_id) ON DELETE CASCADE,
            domain_owner_id TEXT NOT NULL
                REFERENCES scan_domain_owners(domain_owner_id) ON DELETE CASCADE,
            owner_kind TEXT NOT NULL,
            field_id TEXT NOT NULL CHECK (length(field_id) > 0)
        );

CREATE INDEX idx_scan_owner_copies_scan
            ON scan_owner_copies(scan_id, owner_copy_id);

CREATE INDEX idx_scan_owner_copies_domain_owner
            ON scan_owner_copies(domain_owner_id, owner_copy_id);

CREATE TABLE scan_detections (
            scan_id TEXT NOT NULL REFERENCES field_scans(scan_id) ON DELETE CASCADE,
            detection_ordinal INTEGER NOT NULL CHECK (detection_ordinal >= 0),
            exactness TEXT NOT NULL CHECK (exactness = 'exact'),
            -- Labels derive from key names, so the set is open; the shape
            -- stays bounded so a label cannot carry secret text.
            label_id TEXT NOT NULL CHECK (
                length(label_id) BETWEEN 1 AND 64
                AND label_id NOT GLOB '*[^abcdefghijklmnopqrstuvwxyz0-9_]*'
            ),
            span_kind TEXT NOT NULL CHECK (span_kind = 'value'),
            -- 'substitute': the span was replaced before storage.
            -- 'preserve': an existing identity was stored verbatim; rejecting it
            --             retroactively would orphan rows already keyed by it.
            -- 'reject': reserved; rejected writes roll back with their receipts.
            action TEXT NOT NULL CHECK (action IN ('substitute', 'preserve', 'reject')),
            PRIMARY KEY (scan_id, detection_ordinal)
        ) WITHOUT ROWID;
