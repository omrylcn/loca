BEGIN TRANSACTION;
CREATE TABLE attentions (
                    attention_id TEXT PRIMARY KEY,
                    delivery_id TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    source_offset INTEGER NOT NULL,
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    identity TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    priority_rank INTEGER NOT NULL,
                    reply_required INTEGER NOT NULL,
                    context_from_id INTEGER,
                    context_to_id INTEGER,
                    event_json TEXT NOT NULL,
                    bundle_id TEXT NOT NULL REFERENCES bundles(bundle_id),
                    stored_at_ms INTEGER NOT NULL,
                    accepted_at_ms INTEGER,
                    first_response_at_ms INTEGER,
                    final_response_at_ms INTEGER,
                    turn_completed_at_ms INTEGER,
                    terminal_status TEXT CHECK (
                        terminal_status IS NULL OR terminal_status IN (
                            'failed', 'interrupted', 'cancelled', 'expired'
                        )
                    ),
                    terminal_reason TEXT,
                    turn_id TEXT,
                    lease_epoch INTEGER,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
                    UNIQUE (server, room, identity, delivery_id)
                );
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:400','sb-dev:400','reviewer:/var/empty/inbox.jsonl',0,'https://loca.example','sb-dev','reviewer','direct_user',1,1,400,400,'{"id": 400, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 9, "title": "notice-9", "body": "body-9", "from": "operator", "created_at": 1763100000009, "scope": "building"}], "more_available": false}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:400',400000,1794673103422,NULL,NULL,1794673103435,NULL,NULL,'turn-1',1,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:401','sb-dev:401','reviewer:/var/empty/inbox.jsonl',512,'https://loca.example','sb-dev','reviewer','direct_user',1,1,401,401,'{"id": 401, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 9, "title": "notice-9", "body": "body-9", "from": "operator", "created_at": 1763100000009, "scope": "building"}], "more_available": false}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:401',401000,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,0,0);
CREATE TABLE bundle_attentions (
                    bundle_id TEXT NOT NULL REFERENCES bundles(bundle_id),
                    attention_id TEXT NOT NULL REFERENCES attentions(attention_id),
                    position INTEGER NOT NULL,
                    PRIMARY KEY (bundle_id, attention_id),
                    UNIQUE (bundle_id, position)
                );
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:400','attention:reviewer:fc55b34570ce:sb-dev:400',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:401','attention:reviewer:fc55b34570ce:sb-dev:401',0);
CREATE TABLE bundles (
                    bundle_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL
                );
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:400','reviewer','direct_user',1794673103395);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:401','reviewer','direct_user',1794673103462);
CREATE TABLE context_watermarks (
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    last_id INTEGER NOT NULL,
                    observed_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (server, room)
                );
INSERT INTO "context_watermarks" VALUES('https://loca.example','sb-dev',401,1794673103462);
CREATE TABLE dispatch_intents (
                    attention_id TEXT PRIMARY KEY REFERENCES attentions(attention_id),
                    identity TEXT NOT NULL,
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    operation TEXT NOT NULL CHECK (operation IN ('start', 'steer')),
                    client_message_id TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL CHECK (
                        status IN ('prepared', 'accepted', 'reconciled', 'failed')
                    ),
                    turn_id TEXT,
                    prepared_at_ms INTEGER NOT NULL,
                    resolved_at_ms INTEGER,
                    last_error TEXT,
                    lease_epoch INTEGER NOT NULL
                );
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:400','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:400','accepted','turn-1',1794673103410,1794673103422,NULL,1);
CREATE TABLE ingestion_sources (
                    source_id TEXT PRIMARY KEY,
                    path TEXT NOT NULL,
                    offset INTEGER NOT NULL DEFAULT 0,
                    updated_at_ms INTEGER NOT NULL
                );
INSERT INTO "ingestion_sources" VALUES('reviewer:/var/empty/inbox.jsonl','/var/empty/inbox.jsonl',1024,1794673103462);
CREATE TABLE leases (
                    identity TEXT PRIMARY KEY,
                    lease_owner TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    lease_expires_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );
INSERT INTO "leases" VALUES('reviewer','2690794-534edb597831444989253f04e13a7778',1,1794673103464,1794673103464);
CREATE TABLE relay_items (
                    op_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    room TEXT NOT NULL,
                    target TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    item_id TEXT NOT NULL,
                    phase TEXT,
                    text TEXT NOT NULL,
                    covered_attention_ids_json TEXT NOT NULL,
                    relay_kind TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
                    created_at_ms INTEGER NOT NULL,
                    accepted_at_ms INTEGER,
                    lease_epoch INTEGER NOT NULL,
                    UNIQUE (turn_id, item_id, relay_kind)
                );
CREATE TABLE threads (
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    identity TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (server, room, identity),
                    UNIQUE (thread_id)
                );
INSERT INTO "threads" VALUES('https://loca.example','sb-dev','reviewer','thread-1',1,1794673103399,1794673103399);
CREATE TABLE turn_attentions (
                    turn_id TEXT NOT NULL REFERENCES turns(turn_id),
                    attention_id TEXT NOT NULL REFERENCES attentions(attention_id),
                    position INTEGER NOT NULL,
                    PRIMARY KEY (turn_id, attention_id),
                    UNIQUE (turn_id, position)
                );
INSERT INTO "turn_attentions" VALUES('turn-1','attention:reviewer:fc55b34570ce:sb-dev:400',0);
CREATE TABLE turns (
                    turn_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    started_at_ms INTEGER NOT NULL,
                    completed_at_ms INTEGER,
                    terminal_status TEXT,
                    terminal_reason TEXT
                );
INSERT INTO "turns" VALUES('turn-1','reviewer','https://loca.example','sb-dev','thread-1',1,1794673103422,1794673103435,NULL,NULL);
CREATE INDEX attentions_pending
                    ON attentions (
                        identity, terminal_status, accepted_at_ms,
                        next_attempt_at_ms, priority_rank, stored_at_ms
                    );
CREATE INDEX dispatch_intents_unresolved
                    ON dispatch_intents (identity, status, prepared_at_ms);
CREATE INDEX relay_items_pending
                    ON relay_items (identity, status, created_at_ms);
CREATE INDEX relay_items_turn_item
                    ON relay_items (turn_id, item_id);
COMMIT;
