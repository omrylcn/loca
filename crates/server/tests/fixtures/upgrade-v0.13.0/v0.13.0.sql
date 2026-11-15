PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE admin_sessions (
                token TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            );
INSERT INTO "admin_sessions" VALUES('st_034d488be63afafb6e643999f43e1585e0504d1336e2ff047f51fd22c14f29d8','fx-deputy','user',1794720477003);
INSERT INTO "admin_sessions" VALUES('st_6ece506e1e4eda3d8ccd293a7de690ba642e9072064ca68f0178c196dff9b413','Master','user',1826213277083);
CREATE TABLE admission_stock (
                id TEXT PRIMARY KEY,
                minted_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                consumed_at INTEGER,
                consumed_by_name TEXT
            );
INSERT INTO "admission_stock" VALUES('adm_20f9e1db0508e785c0af4379a8a66fd8fc0f6442a3492560','pr_23d4427d9d0de4324b8990ed97233978',1794677278145,1794763678145,1794677278213,'fx-late');
INSERT INTO "admission_stock" VALUES('adm_76dc8fb4c18e8a367a34dbe54f929ca34b4c23fa611a7b93','pr_23d4427d9d0de4324b8990ed97233978',1794677278145,1794763678145,NULL,NULL);
INSERT INTO "admission_stock" VALUES('adm_55eec871ef2b935f1c013035442e3976df13e899cc114696','pr_23d4427d9d0de4324b8990ed97233978',1794677278145,1794763678145,1794677278243,'fx-done');
CREATE TABLE attachment_blobs (
                sha TEXT PRIMARY KEY,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
INSERT INTO "attachment_blobs" VALUES('8f4acefc28453106b9011b2821d4de5db3563157227f948f639d112b3db2175e','text/plain',38,1794677277788);
CREATE TABLE attachment_pending (
                sha TEXT NOT NULL,
                room TEXT NOT NULL,
                uploader TEXT NOT NULL,
                name TEXT NOT NULL,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (sha, room)
            );
CREATE TABLE attachment_refs (
                room TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                sha TEXT NOT NULL,
                name TEXT NOT NULL,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (room, message_id, sha),
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
            );
INSERT INTO "attachment_refs" VALUES('fx-team',9,'8f4acefc28453106b9011b2821d4de5db3563157227f948f639d112b3db2175e','fx-note.txt','text/plain',38,1794677277812);
CREATE TABLE attentions (
                id TEXT PRIMARY KEY,
                room TEXT NOT NULL,
                signal TEXT NOT NULL,
                owner TEXT,
                created_at INTEGER NOT NULL,
                delivered_at INTEGER,
                claimed_by TEXT,
                claimed_at INTEGER,
                resolved_at INTEGER
            );
CREATE TABLE bans (
                room TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,        -- 'ban' | 'mute'
                at INTEGER NOT NULL,
                authority_rank INTEGER NOT NULL DEFAULT 3,
                decided_by TEXT,
                PRIMARY KEY (room, name, kind)
            );
CREATE TABLE birebirs (
                room TEXT PRIMARY KEY,
                a TEXT NOT NULL,
                b TEXT NOT NULL,
                opened_by TEXT NOT NULL,
                opened_at INTEGER NOT NULL,
                master_called_by TEXT,
                master_since INTEGER,
                history_a INTEGER NOT NULL DEFAULT 0,
                history_b INTEGER NOT NULL DEFAULT 0, master_since_ordered INTEGER NOT NULL DEFAULT 0,
                UNIQUE (a, b)
            );
INSERT INTO "birebirs" VALUES('bb-cf41cc795822d31cc407827f05fdde4b','pr_11c9c02fb2fd0b0b4ec47324fbe1d22f','pr_96f31563960808a9d293424182f880df','pr_96f31563960808a9d293424182f880df',1794677277892,NULL,NULL,0,0,0);
CREATE TABLE care_marks (
                room TEXT NOT NULL,
                signal_key TEXT NOT NULL,
                last_signal_at INTEGER NOT NULL,
                signal_count INTEGER NOT NULL,
                PRIMARY KEY (room, signal_key)
            );
CREATE TABLE care_outbox (
                id TEXT PRIMARY KEY,
                attention_id TEXT NOT NULL DEFAULT '',
                delivery_room TEXT NOT NULL,
                owner TEXT NOT NULL,
                owner_principal_id TEXT,
                signal TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                acked_at INTEGER
            );
CREATE TABLE credentials (
                id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL REFERENCES principals(id),
                label TEXT NOT NULL,
                secret_hash TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                revoked_at INTEGER,
                legacy_source TEXT UNIQUE
            );
INSERT INTO "credentials" VALUES('cr_23d4427d9d0de4324b8990ed97233978','pr_23d4427d9d0de4324b8990ed97233978','Root recovery','48cc48104b1837fffa53831f42830d447ae63c9b41ad47073f637f193e3546a7',1794677276403,1794677279119,NULL,'admin:48cc48104b1837fffa53831f42830d447ae63c9b41ad47073f637f193e3546a7');
INSERT INTO "credentials" VALUES('cr_96f31563960808a9d293424182f880df','pr_96f31563960808a9d293424182f880df','Member access','5cd203465f24aa037e0edc55e9b5f54fac7925187078e8ac2071fd4aac298cd2',1794677276543,1794677279081,NULL,'member:5cd203465f24aa037e0edc55e9b5f54fac7925187078e8ac2071fd4aac298cd2');
INSERT INTO "credentials" VALUES('cr_11c9c02fb2fd0b0b4ec47324fbe1d22f','pr_11c9c02fb2fd0b0b4ec47324fbe1d22f','Member access','f79a13056000b5708270c22ba34ac8f7030aef6cff1b2392d368e525f2cf4050',1794677276578,1794677279109,NULL,'member:f79a13056000b5708270c22ba34ac8f7030aef6cff1b2392d368e525f2cf4050');
INSERT INTO "credentials" VALUES('cr_0db74bb3c40c294f1f1d704e468ef1fc','pr_0db74bb3c40c294f1f1d704e468ef1fc','Member access','981716697cf8957bffcacbae545f43b74fa9cba15be804b85d0264284fa65eaf',1794677276630,1794677279020,NULL,'member:981716697cf8957bffcacbae545f43b74fa9cba15be804b85d0264284fa65eaf');
INSERT INTO "credentials" VALUES('cr_893d200a022c822440dc55c124dbbebb','pr_893d200a022c822440dc55c124dbbebb','Member access','396c10903b51fac22a2edf3fdd4f5c120d85fa4f115fd39a78d782c2e78f6eec',1794677276665,NULL,1794677278320,'member:396c10903b51fac22a2edf3fdd4f5c120d85fa4f115fd39a78d782c2e78f6eec');
INSERT INTO "credentials" VALUES('cr_1d74203c684fdfc7f17d915c2dae804b','pr_1d74203c684fdfc7f17d915c2dae804b','Member access','77f01bef7d33c547ae03433cba4fe5b741caf0fd491eb3ef9cc98f4994079615',1794677276705,NULL,NULL,'member:77f01bef7d33c547ae03433cba4fe5b741caf0fd491eb3ef9cc98f4994079615');
INSERT INTO "credentials" VALUES('cr_0bc3f280bfe771b0fc0e6be5b986205f','pr_0bc3f280bfe771b0fc0e6be5b986205f','Smaster access','515849ac7019301d4b0764362fb2af6539a93e5fef21758bc203383c0285a176',1794677276793,1794677278493,NULL,'smaster:515849ac7019301d4b0764362fb2af6539a93e5fef21758bc203383c0285a176');
INSERT INTO "credentials" VALUES('cr_33944c9ed7750a56930956ae60b48f56','pr_33944c9ed7750a56930956ae60b48f56','Smaster access','37a002311d9ff6eeec93a090672962744e46bafe2075ab030dac1e327c2de3b8',1794677276826,NULL,NULL,'smaster:37a002311d9ff6eeec93a090672962744e46bafe2075ab030dac1e327c2de3b8');
INSERT INTO "credentials" VALUES('cr_99acea781c6c523d7d7def27ca0417f7','pr_99acea781c6c523d7d7def27ca0417f7','Smaster access','d763f4d82a249f2d063807272602324f4be0a361594b430fb7800f90cbee3f38',1794677276876,NULL,1794677276952,'smaster:d763f4d82a249f2d063807272602324f4be0a361594b430fb7800f90cbee3f38');
INSERT INTO "credentials" VALUES('cr_bbd256609d257a960e7f917cf16e82c6','pr_bbd256609d257a960e7f917cf16e82c6','Member access','2918de71b2e16e0f9cad2eeb11fc3248df8d4ddcb712f731bb6e136a7941e546',1794677278213,1794677278678,NULL,'member:2918de71b2e16e0f9cad2eeb11fc3248df8d4ddcb712f731bb6e136a7941e546');
INSERT INTO "credentials" VALUES('cr_69909ccd5742fc357f78b871530985fc','pr_69909ccd5742fc357f78b871530985fc','Member access','2c78d958cfd1a5cb8206bba4fa26edbbb0010b8e24b362dd16351d653cbee3b2',1794677278243,1794677278718,NULL,'member:2c78d958cfd1a5cb8206bba4fa26edbbb0010b8e24b362dd16351d653cbee3b2');
CREATE TABLE goals (
                id INTEGER NOT NULL,
                room TEXT NOT NULL,
                outcome TEXT NOT NULL,
                checkpoint TEXT,
                stale_after_secs INTEGER,
                created_by TEXT NOT NULL,
                completion TEXT NOT NULL,
                task_ids TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                progress_at INTEGER NOT NULL,
                closed_at INTEGER,
                PRIMARY KEY (room, id)
            );
INSERT INTO "goals" VALUES(1,'fx-team','fx the upgrade opens',NULL,NULL,'Master','manual','[]','active',1794677278093,1794677278093,NULL);
CREATE TABLE group_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        group_id TEXT NOT NULL,
        action TEXT NOT NULL,
        subject TEXT,
        by_principal TEXT,
        at INTEGER NOT NULL
    );
INSERT INTO "group_log" VALUES(1,'grp_af3c92e2f9b643d3','created','fx-group','pr_23d4427d9d0de4324b8990ed97233978',1794677277920);
INSERT INTO "group_log" VALUES(2,'grp_af3c92e2f9b643d3','member_added','pr_96f31563960808a9d293424182f880df','pr_23d4427d9d0de4324b8990ed97233978',1794677277952);
INSERT INTO "group_log" VALUES(3,'grp_af3c92e2f9b643d3','member_added','pr_0db74bb3c40c294f1f1d704e468ef1fc','pr_23d4427d9d0de4324b8990ed97233978',1794677277962);
INSERT INTO "group_log" VALUES(4,'grp_af3c92e2f9b643d3','loca_joined','fx-ops','pr_23d4427d9d0de4324b8990ed97233978',1794677277970);
CREATE TABLE group_members (
        group_id TEXT NOT NULL REFERENCES groups(id),
        principal_id TEXT NOT NULL,
        added_by TEXT,
        added_at INTEGER NOT NULL,
        PRIMARY KEY (group_id, principal_id)
    );
INSERT INTO "group_members" VALUES('grp_af3c92e2f9b643d3','pr_96f31563960808a9d293424182f880df','pr_23d4427d9d0de4324b8990ed97233978',1794677277952);
INSERT INTO "group_members" VALUES('grp_af3c92e2f9b643d3','pr_0db74bb3c40c294f1f1d704e468ef1fc','pr_23d4427d9d0de4324b8990ed97233978',1794677277962);
CREATE TABLE group_rooms (
        room TEXT PRIMARY KEY,
        group_id TEXT NOT NULL REFERENCES groups(id),
        set_by TEXT,
        set_at INTEGER NOT NULL
    );
INSERT INTO "group_rooms" VALUES('fx-ops','grp_af3c92e2f9b643d3','pr_23d4427d9d0de4324b8990ed97233978',1794677277970);
CREATE TABLE groups (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        founder TEXT NOT NULL,
        owner TEXT,
        created_at INTEGER NOT NULL,
        archived_at INTEGER,
        deleted_at INTEGER
    );
INSERT INTO "groups" VALUES('grp_af3c92e2f9b643d3','fx-group','pr_23d4427d9d0de4324b8990ed97233978',NULL,1794677277920,NULL,NULL);
CREATE TABLE invites (
                token TEXT PRIMARY KEY,
                room TEXT NOT NULL,        -- hangi loca; başka locayı açmaz
                name TEXT NOT NULL,        -- kime verildi
                kind TEXT NOT NULL,        -- 'agent' | 'user'
                issued_at INTEGER NOT NULL,
                issued_by TEXT NOT NULL,   -- 'master'
                revoked_at INTEGER         -- NULL = geçerli
            , member TEXT NOT NULL DEFAULT '');
INSERT INTO "invites" VALUES('dv_8cee1244f040fc5af616c4d493042c51da8094b4a6e2251fdaecff3791f75f38','fx-team','fx-ada','user',1794677277224,'master',NULL,'mb_1c312018eff20476dec1f2b937b1bd78ddac15a4c9dd994fda99c8a187dc7daf');
INSERT INTO "invites" VALUES('dv_b0a05bf1e0d59a85b1144802aeacb27a9f074ffdc0fe4e82cf019722d1f969cb','fx-team','fx-bea','user',1794677277267,'master',NULL,'mb_849320f90e7cb877e7e50068b64d8ecf920ed9b6016aacd0c6b249499389717b');
INSERT INTO "invites" VALUES('dv_ec5905fed01ab24f8d460321542a5877420cb0c9db8a042166d386073d466e41','fx-team','fx-bot','agent',1794677277328,'master',NULL,'mb_e9cc197a60ca3b1ea0f810d585779b75634eb134707da4f1904ec0ab9bca4c44');
INSERT INTO "invites" VALUES('dv_53a9cd1eb73fa392c3b6b45c6107288b546ebb2bb776c5f60dae5dac05838542','fx-team','fx-paused','user',1794677277379,'master',1794677278349,'mb_dee7e42562e269397eafc868be2544a53bc7b3ebf18717a21a25881fd67b85dc');
INSERT INTO "invites" VALUES('dv_c3a35e5cf54fa5d535ef8047045d10fc0bb0f82f11ce689a4961bb6bf236a3b8','fx-ops','fx-ada','user',1794677277443,'master',NULL,'mb_1c312018eff20476dec1f2b937b1bd78ddac15a4c9dd994fda99c8a187dc7daf');
INSERT INTO "invites" VALUES('dv_0442fca4aef042747ef18fd0702e480c98cfda9fd039aeeb9c57190c3823879b','fx-ops','fx-bot','agent',1794677277499,'master',NULL,'mb_e9cc197a60ca3b1ea0f810d585779b75634eb134707da4f1904ec0ab9bca4c44');
INSERT INTO "invites" VALUES('dv_4d4ee16d7692b0feb8e0f76a760f954a618126c86c6c6bf965b218870f30400c','fx-ops','fx-bea','user',1794677277546,'master',1794677277641,'mb_849320f90e7cb877e7e50068b64d8ecf920ed9b6016aacd0c6b249499389717b');
CREATE TABLE join_requests (
                id TEXT PRIMARY KEY,
                secret_hash TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                status TEXT NOT NULL
                    CHECK (status IN ('pending', 'approving', 'approved', 'denied')),
                created_at INTEGER NOT NULL,
                decided_at INTEGER,
                decided_by TEXT,
                mb_token TEXT,
                bootstrap_delivered_at INTEGER
            , bootstrap_acked_at INTEGER);
INSERT INTO "join_requests" VALUES('jr_cffd3684a59587b56aca2abc','4e767ed7b6717fd3b91f1977484d3eee7013460103b66b07e2c7cc44938a0301','fx-wait','agent','pending',1794677278149,NULL,NULL,NULL,NULL,NULL);
INSERT INTO "join_requests" VALUES('jr_88c26fbe83305961bb9cf554','ef4dccc4be0f942093c1a6262b58739ffb3be0a18480165fe032a87cb87781cd','fx-late','agent','approved',1794677278162,1794677278213,'master','mb_1745e8b70fd5f9e7d8c1153545e2d4050f5381dad115c98a258698be18000d71',1794677278217,NULL);
INSERT INTO "join_requests" VALUES('jr_ce2e06dcfb841adbf72a172d','94ddc92f572a460edd41ac8d776ae0a3ae5f1554f737eca7fd8b3c076b151412','fx-done','agent','approved',1794677278176,1794677278243,'master',NULL,1794677278256,1794677278266);
INSERT INTO "join_requests" VALUES('jr_0e0c87fe48fd01d59e47f044','62e60b06cc56c814349102ebb2cf1375072ae3f3def926e22d2083afe07370a0','fx-no','agent','denied',1794677278190,1794677278279,'master',NULL,NULL,NULL);
CREATE TABLE journal (
                id INTEGER NOT NULL,
                room TEXT NOT NULL,
                by TEXT NOT NULL,
                by_type TEXT NOT NULL,
                text TEXT NOT NULL,
                at INTEGER NOT NULL,
                PRIMARY KEY (room, id)
            );
INSERT INTO "journal" VALUES(1,'fx-ops','Master','user','moved this loca to fx-group',1794677277983);
INSERT INTO "journal" VALUES(1,'fx-team','Master','user','extensions: removed Loca Wiki',1794677278048);
INSERT INTO "journal" VALUES(2,'fx-team','fx-bot','agent','fx bot finished the fixture',1794677278067);
CREATE TABLE loca_memory (
                room TEXT PRIMARY KEY,
                owner TEXT,
                short TEXT NOT NULL DEFAULT '',
                long TEXT NOT NULL DEFAULT '',
                short_updated_at INTEGER,
                long_updated_at INTEGER,
                version INTEGER NOT NULL DEFAULT 0
            , owner_principal_id TEXT REFERENCES principals(id));
CREATE TABLE loca_memory_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                room TEXT NOT NULL,
                text TEXT NOT NULL,
                decided_by TEXT NOT NULL,
                decided_at INTEGER NOT NULL
            );
CREATE TABLE loca_wiki (
                room TEXT PRIMARY KEY,
                editor_principal TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 0,
                interval_messages INTEGER NOT NULL DEFAULT 30,
                revision INTEGER NOT NULL DEFAULT 0,
                reviewed_through INTEGER NOT NULL DEFAULT 0,
                reviewed_at INTEGER,
                edited_at INTEGER
            );
CREATE TABLE loca_wiki_batches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                room TEXT NOT NULL,
                editor_principal TEXT NOT NULL,
                after_id INTEGER NOT NULL,
                through_id INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                reason TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                epoch INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0,
                lease_expires_at INTEGER NOT NULL DEFAULT 0
            );
CREATE TABLE loca_wiki_history (
                room TEXT NOT NULL,
                slug TEXT NOT NULL,
                revision INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                sources TEXT NOT NULL,
                editor_principal TEXT NOT NULL,
                reason TEXT NOT NULL,
                edited_at INTEGER NOT NULL,
                PRIMARY KEY(room, slug, revision)
            );
CREATE TABLE loca_wiki_leases (
                batch_id INTEGER NOT NULL,
                epoch INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                PRIMARY KEY(batch_id, epoch)
            );
CREATE TABLE loca_wiki_pages (
                room TEXT NOT NULL,
                slug TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                sources TEXT NOT NULL,
                revision INTEGER NOT NULL,
                PRIMARY KEY(room, slug)
            );
CREATE TABLE loca_wiki_receipts (
                room TEXT NOT NULL,
                batch_id INTEGER NOT NULL,
                epoch INTEGER NOT NULL,
                editor_principal TEXT NOT NULL,
                request TEXT NOT NULL,
                revision INTEGER NOT NULL,
                PRIMARY KEY(room, batch_id, epoch)
            );
CREATE TABLE member_suspensions (
                member_token TEXT PRIMARY KEY REFERENCES members(token),
                suspended_at INTEGER NOT NULL,
                reason TEXT NOT NULL
            );
INSERT INTO "member_suspensions" VALUES('mb_dee7e42562e269397eafc868be2544a53bc7b3ebf18717a21a25881fd67b85dc',1794677278349,'');
CREATE TABLE members (
                token TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                joined_at INTEGER NOT NULL,
                admitted_by TEXT NOT NULL,
                revoked_at INTEGER
            );
INSERT INTO "members" VALUES('mb_1c312018eff20476dec1f2b937b1bd78ddac15a4c9dd994fda99c8a187dc7daf','fx-ada','user',1794677276543,'master',NULL);
INSERT INTO "members" VALUES('mb_849320f90e7cb877e7e50068b64d8ecf920ed9b6016aacd0c6b249499389717b','fx-bea','user',1794677276578,'master',NULL);
INSERT INTO "members" VALUES('mb_e9cc197a60ca3b1ea0f810d585779b75634eb134707da4f1904ec0ab9bca4c44','fx-bot','agent',1794677276630,'master',NULL);
INSERT INTO "members" VALUES('mb_27619fe3a755e89a3296358f0dfc4bccfb5fbad25f877b9a3caee3b0f44e9731','fx-gone','agent',1794677276665,'master',1794677278320);
INSERT INTO "members" VALUES('mb_dee7e42562e269397eafc868be2544a53bc7b3ebf18717a21a25881fd67b85dc','fx-paused','user',1794677276705,'master',NULL);
INSERT INTO "members" VALUES('mb_1745e8b70fd5f9e7d8c1153545e2d4050f5381dad115c98a258698be18000d71','fx-late','agent',1794677278213,'master',NULL);
INSERT INTO "members" VALUES('mb_053eb83f6ecab7706fd8a00a2d01fcca6cbb486903320af6b256793f4bdcdfea','fx-done','agent',1794677278243,'master',NULL);
CREATE TABLE message_reactions (
                room TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                principal TEXT NOT NULL,
                reactor TEXT NOT NULL,
                emoji TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (room, message_id, principal, emoji),
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
            );
CREATE TABLE messages (
                id INTEGER PRIMARY KEY,
                room TEXT NOT NULL,
                sender TEXT NOT NULL,
                sender_type TEXT NOT NULL,
                target TEXT,
                text TEXT NOT NULL,
                reply_to INTEGER,
                ts INTEGER NOT NULL,
                kind TEXT NOT NULL DEFAULT 'say'
            , principal TEXT, op_id TEXT, attachments TEXT);
INSERT INTO "messages" VALUES(6,'fx-team','fx-ada','user',NULL,'fx hello from ada',NULL,1794677277769,'say',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(7,'fx-team','fx-bot','agent',NULL,'fx status from bot',NULL,1794677277774,'say','agent:fx-bot','fx-op-bot-0001',NULL);
INSERT INTO "messages" VALUES(8,'fx-team','fx-bea','user',NULL,'fx reply from bea',6,1794677277783,'say',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(9,'fx-team','fx-ada','user',NULL,'fx file from ada',NULL,1794677277812,'say','user:fx-ada','fx-op-ada-0001','[{"id":"8f4acefc28453106b9011b2821d4de5db3563157227f948f639d112b3db2175e","sha256":"8f4acefc28453106b9011b2821d4de5db3563157227f948f639d112b3db2175e","name":"fx-note.txt","mime":"text/plain","size":38}]');
INSERT INTO "messages" VALUES(10,'fx-team','Master','user',NULL,'fx word from the master',NULL,1794677277820,'say',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(12,'fx-team','fx-deputy','user',NULL,'fx socket word from deputy',NULL,1794677277825,'say','sm:pr:pr_0bc3f280bfe771b0fc0e6be5b986205f','fx-op-deputy-0001',NULL);
INSERT INTO "messages" VALUES(13,'bb-cf41cc795822d31cc407827f05fdde4b','fx-ada','user','fx-bea','fx private ada to bea',NULL,1794677277903,'say',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(14,'bb-cf41cc795822d31cc407827f05fdde4b','fx-bea','user','fx-ada','fx private bea to ada',NULL,1794677277908,'say',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(15,'fx-team','loca','user','fx-bot','Master set @lead fx-bot.',NULL,1794677278082,'announce',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(16,'iye','loca','user','all','fx-wait (agent) wants to join — approve or deny it in Join requests.',NULL,1794677278158,'announce',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(17,'iye','loca','user','all','fx-late (agent) wants to join — approve or deny it in Join requests.',NULL,1794677278172,'announce',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(18,'iye','loca','user','all','fx-done (agent) wants to join — approve or deny it in Join requests.',NULL,1794677278186,'announce',NULL,NULL,NULL);
INSERT INTO "messages" VALUES(19,'iye','loca','user','all','fx-no (agent) wants to join — approve or deny it in Join requests.',NULL,1794677278199,'announce',NULL,NULL,NULL);
CREATE TABLE note_revisions (
                room TEXT NOT NULL,
                key TEXT NOT NULL,
                rev INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                updated_by TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (room, key, rev)
            );
INSERT INTO "note_revisions" VALUES('fx-team','fx-plan',1,'Plan','fx plan v1','fx-ada',1794677278052);
CREATE TABLE notes (
                room TEXT NOT NULL,
                key TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                can_write TEXT NOT NULL,   -- JSON array
                updated_by TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                rev INTEGER NOT NULL,
                PRIMARY KEY (room, key)
            );
INSERT INTO "notes" VALUES('fx-team','fx-plan','Plan','fx plan v2','[]','fx-ada',1794677278063,2);
CREATE TABLE notification_receipts (
        notification_id INTEGER NOT NULL REFERENCES notifications(id),
        principal_id TEXT NOT NULL,
        read_at INTEGER,
        PRIMARY KEY (notification_id, principal_id)
    );
INSERT INTO "notification_receipts" VALUES(1,'pr_0bc3f280bfe771b0fc0e6be5b986205f',NULL);
INSERT INTO "notification_receipts" VALUES(1,'pr_0db74bb3c40c294f1f1d704e468ef1fc',NULL);
INSERT INTO "notification_receipts" VALUES(1,'pr_11c9c02fb2fd0b0b4ec47324fbe1d22f',NULL);
INSERT INTO "notification_receipts" VALUES(1,'pr_1d74203c684fdfc7f17d915c2dae804b',NULL);
INSERT INTO "notification_receipts" VALUES(1,'pr_33944c9ed7750a56930956ae60b48f56',NULL);
INSERT INTO "notification_receipts" VALUES(1,'pr_893d200a022c822440dc55c124dbbebb',NULL);
INSERT INTO "notification_receipts" VALUES(1,'pr_96f31563960808a9d293424182f880df',1794677278037);
CREATE TABLE notifications (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        sender_principal TEXT NOT NULL,
        sender_name TEXT NOT NULL,
        audience TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
INSERT INTO "notifications" VALUES(1,'pr_23d4427d9d0de4324b8990ed97233978','Master','{"all":true}','fx notice','fx notice body',1794677278033);
CREATE TABLE pairing_log (
                id TEXT PRIMARY KEY,
                fingerprint TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                session_ttl_ms INTEGER NOT NULL,
                used_at INTEGER,
                superseded_at INTEGER
            );
INSERT INTO "pairing_log" VALUES('pl_50e06afb13bdf6fffb5adda5','pair_5ba4…',1794677277069,1794677577069,31536000000,1794677277073,NULL);
CREATE TABLE principal_preferences (
                principal_id TEXT PRIMARY KEY REFERENCES principals(id),
                json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
CREATE TABLE principal_sessions (
                id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL REFERENCES principals(id),
                credential_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                revoked_at INTEGER,
                FOREIGN KEY (credential_id, principal_id)
                    REFERENCES credentials(id, principal_id)
            );
INSERT INTO "principal_sessions" VALUES('bb02908b96590966c7da58ed38b98dd82f8f9e9fe399db938c92d3ff4b225909','pr_0bc3f280bfe771b0fc0e6be5b986205f','cr_0bc3f280bfe771b0fc0e6be5b986205f',1794677277016,1794720477003,NULL);
INSERT INTO "principal_sessions" VALUES('f1d96856cb727b6b36634602787b25a5ebd83881aab4f0739dd89101980a1624','pr_23d4427d9d0de4324b8990ed97233978','cr_23d4427d9d0de4324b8990ed97233978',1794677277086,1826213277083,NULL);
INSERT INTO "principal_sessions" VALUES('a17e0cec96436b9b67fd3cd3c502d63751ee3258424ecccd54893c9e0e6a8fc7','pr_96f31563960808a9d293424182f880df','cr_96f31563960808a9d293424182f880df',1794677277671,1794720477671,NULL);
INSERT INTO "principal_sessions" VALUES('b4091bf1f8f3387a361a90b3b7186895175c42485f18e5d3fb6ef2929556abef','pr_11c9c02fb2fd0b0b4ec47324fbe1d22f','cr_11c9c02fb2fd0b0b4ec47324fbe1d22f',1794677277727,1794720477727,NULL);
INSERT INTO "principal_sessions" VALUES('68de3e580fbb2c23686baceba0f7b09647a87e3cebcffbb8d81b8c6957d22304','pr_0db74bb3c40c294f1f1d704e468ef1fc','cr_0db74bb3c40c294f1f1d704e468ef1fc',1794677277760,1794720477760,NULL);
CREATE TABLE principals (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('human', 'agent')),
                building_role TEXT NOT NULL CHECK (building_role IN ('master', 'smaster', 'member')),
                created_at INTEGER NOT NULL,
                disabled_at INTEGER
            );
INSERT INTO "principals" VALUES('pr_23d4427d9d0de4324b8990ed97233978','Master','human','master',1794677276403,NULL);
INSERT INTO "principals" VALUES('pr_96f31563960808a9d293424182f880df','fx-ada','human','member',1794677276543,NULL);
INSERT INTO "principals" VALUES('pr_11c9c02fb2fd0b0b4ec47324fbe1d22f','fx-bea','human','member',1794677276578,NULL);
INSERT INTO "principals" VALUES('pr_0db74bb3c40c294f1f1d704e468ef1fc','fx-bot','agent','member',1794677276630,NULL);
INSERT INTO "principals" VALUES('pr_893d200a022c822440dc55c124dbbebb','fx-gone','agent','member',1794677276665,1794677278320);
INSERT INTO "principals" VALUES('pr_1d74203c684fdfc7f17d915c2dae804b','fx-paused','human','member',1794677276705,1794677278349);
INSERT INTO "principals" VALUES('pr_0bc3f280bfe771b0fc0e6be5b986205f','fx-deputy','human','smaster',1794677276793,NULL);
INSERT INTO "principals" VALUES('pr_33944c9ed7750a56930956ae60b48f56','fx-spare','human','smaster',1794677276826,NULL);
INSERT INTO "principals" VALUES('pr_99acea781c6c523d7d7def27ca0417f7','fx-retired','human','smaster',1794677276876,1794677276952);
INSERT INTO "principals" VALUES('pr_bbd256609d257a960e7f917cf16e82c6','fx-late','agent','member',1794677278213,NULL);
INSERT INTO "principals" VALUES('pr_69909ccd5742fc357f78b871530985fc','fx-done','agent','member',1794677278243,NULL);
CREATE TABLE room_operator_assignments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                room TEXT NOT NULL,
                principal_id TEXT NOT NULL REFERENCES principals(id),
                appointed_by_principal_id TEXT NOT NULL REFERENCES principals(id),
                appointed_by_role TEXT NOT NULL
                    CHECK (appointed_by_role IN ('master', 'smaster', 'member')),
                appointed_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
INSERT INTO "room_operator_assignments" VALUES(1,'fx-team','pr_23d4427d9d0de4324b8990ed97233978','pr_23d4427d9d0de4324b8990ed97233978','master',1794677277130,NULL);
INSERT INTO "room_operator_assignments" VALUES(2,'fx-ops','pr_23d4427d9d0de4324b8990ed97233978','pr_23d4427d9d0de4324b8990ed97233978','master',1794677277173,NULL);
CREATE TABLE rooms (
                room TEXT PRIMARY KEY,
                mode TEXT NOT NULL,        -- JSON ChatMode
                settings TEXT NOT NULL     -- JSON RoomSettings
            , sealed_at INTEGER);
INSERT INTO "rooms" VALUES('fx-team','{"mode":"free"}','{"lead":"fx-bot","rate_limit":10,"rate_window_secs":30,"live":false,"archived":false,"live_timeout_secs":120,"operators":[],"turn_max_messages":4,"turn_idle_ms":5000,"turn_max_wait_ms":15000,"care_wait_secs":120,"care_cooldown_secs":300,"care_max_attempts":2,"care_context_messages":8,"care_recipient":{"kind":"lead"},"care_goal_secs":0,"care_task_secs":0,"care_silence_secs":0,"extensions":["notes","focus","journal","live"]}',NULL);
INSERT INTO "rooms" VALUES('fx-ops','{"mode":"free"}','{"rate_limit":10,"rate_window_secs":30,"live":false,"archived":false,"live_timeout_secs":120,"operators":[],"turn_max_messages":4,"turn_idle_ms":5000,"turn_max_wait_ms":15000,"care_wait_secs":120,"care_cooldown_secs":300,"care_max_attempts":2,"care_context_messages":8,"care_recipient":{"kind":"lead"},"care_goal_secs":0,"care_task_secs":0,"care_silence_secs":0}',NULL);
INSERT INTO "rooms" VALUES('bb-cf41cc795822d31cc407827f05fdde4b','{"mode":"free"}','{"rate_limit":10,"rate_window_secs":30,"live":false,"archived":false,"live_timeout_secs":120,"operators":[],"turn_max_messages":4,"turn_idle_ms":5000,"turn_max_wait_ms":15000,"care_wait_secs":120,"care_cooldown_secs":300,"care_max_attempts":2,"care_context_messages":8,"care_recipient":{"kind":"lead"},"care_goal_secs":0,"care_task_secs":0,"care_silence_secs":0,"extensions":[]}',NULL);
INSERT INTO "rooms" VALUES('iye','{"mode":"free"}','{"rate_limit":10,"rate_window_secs":30,"live":false,"archived":false,"live_timeout_secs":120,"operators":[],"turn_max_messages":4,"turn_idle_ms":5000,"turn_max_wait_ms":15000,"care_wait_secs":120,"care_cooldown_secs":300,"care_max_attempts":2,"care_context_messages":8,"care_recipient":{"kind":"lead"},"care_goal_secs":0,"care_task_secs":0,"care_silence_secs":0}',NULL);
CREATE TABLE smasters (
                token TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
INSERT INTO "smasters" VALUES('sm_ab8f88facf41eed7ae142a6e84f2214cef6e60dfa6ac33464b2fb352e839513a','fx-deputy',1794677276793,NULL);
INSERT INTO "smasters" VALUES('sm_0480fc02bc43cab0725eb0395f74b63d39ee9b6aa42e3482f542af735f6d1b6d','fx-spare',1794677276826,NULL);
INSERT INTO "smasters" VALUES('sm_e40032bb0f20cc66e5adcb6f55e323983c91732958f43ce6877c3e7640677a14','fx-retired',1794677276876,1794677276952);
CREATE TABLE tasks (
                id INTEGER NOT NULL,
                room TEXT NOT NULL,
                title TEXT NOT NULL,
                created_by TEXT NOT NULL,
                from_message INTEGER,
                assigned_to TEXT,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                progress_at INTEGER NOT NULL,
                closed_at INTEGER,
                PRIMARY KEY (room, id)
            );
INSERT INTO "tasks" VALUES(1,'fx-team','fx write the upgrade test','Master',NULL,'fx-bot','open',1794677278078,1794677278078,NULL);
CREATE TABLE waits (
                room TEXT NOT NULL,
                waiter TEXT NOT NULL,
                waiting_for TEXT NOT NULL,
                reason TEXT NOT NULL,
                since INTEGER NOT NULL,
                last_signal_at INTEGER,
                signal_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (room, waiter)
            );
CREATE TABLE wiki_context_receipts (
                room TEXT NOT NULL, principal TEXT NOT NULL, context_id TEXT NOT NULL,
                packet_id TEXT NOT NULL, revision INTEGER NOT NULL,
                selection_version INTEGER NOT NULL, accepted_at INTEGER NOT NULL,
                PRIMARY KEY(room, principal, context_id, packet_id)
            );
CREATE TABLE wiki_context_selection (
                room TEXT NOT NULL, principal TEXT NOT NULL, slugs TEXT NOT NULL,
                reason TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 1,
                requested_at INTEGER NOT NULL, PRIMARY KEY(room, principal)
            );
CREATE TABLE wiki_native_runtime (
                room TEXT NOT NULL, principal TEXT NOT NULL, runtime TEXT NOT NULL,
                ready_at INTEGER NOT NULL, PRIMARY KEY(room, principal)
            );
CREATE INDEX idx_messages_room ON messages(room, id);
CREATE INDEX message_reactions_room
                ON message_reactions(room, message_id);
CREATE INDEX loca_memory_entries_room
                ON loca_memory_entries(room, id);
CREATE UNIQUE INDEX wiki_one_active_batch
                ON loca_wiki_batches(room)
                WHERE status IN ('queued', 'running', 'blocked');
CREATE UNIQUE INDEX goals_one_active
                ON goals(room) WHERE status = 'active';
CREATE INDEX care_outbox_delivery
                ON care_outbox(delivery_room, owner, acked_at);
CREATE INDEX attentions_room
                ON attentions(room, created_at, id);
CREATE INDEX invites_room ON invites(room);
CREATE INDEX journal_room ON journal(room, id);
CREATE UNIQUE INDEX principals_one_master
                ON principals(building_role) WHERE building_role = 'master' AND disabled_at IS NULL;
CREATE INDEX credentials_principal
                ON credentials(principal_id, revoked_at);
CREATE UNIQUE INDEX credentials_owner
                ON credentials(id, principal_id);
CREATE INDEX principal_sessions_principal
                ON principal_sessions(principal_id, revoked_at, expires_at);
CREATE UNIQUE INDEX room_operator_one_active
                ON room_operator_assignments(room) WHERE revoked_at IS NULL;
CREATE INDEX room_operator_history
                ON room_operator_assignments(room, appointed_at, id);
CREATE INDEX admission_stock_available
                ON admission_stock(consumed_at, expires_at);
CREATE INDEX join_requests_pending
                ON join_requests(status, created_at);
CREATE INDEX attachment_refs_room_sha
                ON attachment_refs(room, sha);
CREATE INDEX attachment_refs_sha
                ON attachment_refs(sha);
CREATE INDEX group_log_group ON group_log(group_id, id);
CREATE INDEX notifications_sender
        ON notifications(sender_principal, id);
CREATE INDEX notification_receipts_principal
        ON notification_receipts(principal_id, notification_id);
CREATE UNIQUE INDEX idx_messages_operation
             ON messages(room, principal, op_id)
             WHERE principal IS NOT NULL AND op_id IS NOT NULL;
CREATE INDEX care_outbox_delivery_principal
                ON care_outbox(delivery_room, owner_principal_id, acked_at);
DELETE FROM "sqlite_sequence";
INSERT INTO "sqlite_sequence" VALUES('room_operator_assignments',2);
INSERT INTO "sqlite_sequence" VALUES('group_log',4);
INSERT INTO "sqlite_sequence" VALUES('notifications',1);
COMMIT;
