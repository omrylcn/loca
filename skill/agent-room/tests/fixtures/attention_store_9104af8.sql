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
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:400','sb-dev:400','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',0,'https://loca.example','sb-dev','reviewer','direct_user',1,1,400,400,'{"id": 400, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 1, "title": "notice-1", "body": "body-1", "from": "operator", "created_at": 1763100000001, "scope": "building"}, {"id": 2, "title": "notice-2", "body": "body-2", "from": "operator", "created_at": 1763100000002, "scope": "building"}, {"id": 3, "title": "notice-3", "body": "body-3", "from": "operator", "created_at": 1763100000003, "scope": "building"}, {"id": 4, "title": "notice-4", "body": "body-4", "from": "operator", "created_at": 1763100000004, "scope": "building"}], "more_available": false}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:400',400000,1794680184742,NULL,NULL,1794680184749,NULL,NULL,'turn-1',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:401','sb-dev:401','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',872,'https://loca.example','sb-dev','reviewer','direct_user',1,1,401,401,'{"id": 401, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 5, "title": "notice-5", "body": "body-5", "from": "operator", "created_at": 1763100000005, "scope": "building"}, {"id": 6, "title": "notice-6", "body": "body-6", "from": "operator", "created_at": 1763100000006, "scope": "building"}, {"id": 7, "title": "notice-7", "body": "body-7", "from": "operator", "created_at": 1763100000007, "scope": "building"}, {"id": 8, "title": "notice-8", "body": "body-8", "from": "operator", "created_at": 1763100000008, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:401',401000,1794680184762,NULL,NULL,1794680184768,NULL,NULL,'turn-2',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:402','sb-dev:402','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',1743,'https://loca.example','sb-dev','reviewer','direct_user',1,1,402,402,'{"id": 402, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 9, "title": "notice-9", "body": "body-9", "from": "operator", "created_at": 1763100000009, "scope": "building"}, {"id": 10, "title": "notice-10", "body": "body-10", "from": "operator", "created_at": 1763100000010, "scope": "building"}, {"id": 11, "title": "notice-11", "body": "body-11", "from": "operator", "created_at": 1763100000011, "scope": "building"}, {"id": 12, "title": "notice-12", "body": "body-12", "from": "operator", "created_at": 1763100000012, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:402',402000,1794680184787,NULL,NULL,1794680184788,NULL,NULL,'turn-3',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:403','sb-dev:403','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',2623,'https://loca.example','sb-dev','reviewer','direct_user',1,1,403,403,'{"id": 403, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 13, "title": "notice-13", "body": "body-13", "from": "operator", "created_at": 1763100000013, "scope": "building"}, {"id": 14, "title": "notice-14", "body": "body-14", "from": "operator", "created_at": 1763100000014, "scope": "building"}, {"id": 15, "title": "notice-15", "body": "body-15", "from": "operator", "created_at": 1763100000015, "scope": "building"}, {"id": 16, "title": "notice-16", "body": "body-16", "from": "operator", "created_at": 1763100000016, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:403',403000,1794680184802,NULL,NULL,1794680184803,NULL,NULL,'turn-4',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:404','sb-dev:404','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',3506,'https://loca.example','sb-dev','reviewer','direct_user',1,1,404,404,'{"id": 404, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 17, "title": "notice-17", "body": "body-17", "from": "operator", "created_at": 1763100000017, "scope": "building"}, {"id": 18, "title": "notice-18", "body": "body-18", "from": "operator", "created_at": 1763100000018, "scope": "building"}, {"id": 19, "title": "notice-19", "body": "body-19", "from": "operator", "created_at": 1763100000019, "scope": "building"}, {"id": 20, "title": "notice-20", "body": "body-20", "from": "operator", "created_at": 1763100000020, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:404',404000,1794680184821,NULL,NULL,1794680184823,NULL,NULL,'turn-5',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:405','sb-dev:405','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',4389,'https://loca.example','sb-dev','reviewer','direct_user',1,1,405,405,'{"id": 405, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 21, "title": "notice-21", "body": "body-21", "from": "operator", "created_at": 1763100000021, "scope": "building"}, {"id": 22, "title": "notice-22", "body": "body-22", "from": "operator", "created_at": 1763100000022, "scope": "building"}, {"id": 23, "title": "notice-23", "body": "body-23", "from": "operator", "created_at": 1763100000023, "scope": "building"}, {"id": 24, "title": "notice-24", "body": "body-24", "from": "operator", "created_at": 1763100000024, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:405',405000,1794680184837,NULL,NULL,1794680184844,NULL,NULL,'turn-6',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:406','sb-dev:406','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',5272,'https://loca.example','sb-dev','reviewer','direct_user',1,1,406,406,'{"id": 406, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 25, "title": "notice-25", "body": "body-25", "from": "operator", "created_at": 1763100000025, "scope": "building"}, {"id": 26, "title": "notice-26", "body": "body-26", "from": "operator", "created_at": 1763100000026, "scope": "building"}, {"id": 27, "title": "notice-27", "body": "body-27", "from": "operator", "created_at": 1763100000027, "scope": "building"}, {"id": 28, "title": "notice-28", "body": "body-28", "from": "operator", "created_at": 1763100000028, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:406',406000,1794680184859,NULL,NULL,1794680184866,NULL,NULL,'turn-7',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:407','sb-dev:407','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',6155,'https://loca.example','sb-dev','reviewer','direct_user',1,1,407,407,'{"id": 407, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 29, "title": "notice-29", "body": "body-29", "from": "operator", "created_at": 1763100000029, "scope": "building"}, {"id": 30, "title": "notice-30", "body": "body-30", "from": "operator", "created_at": 1763100000030, "scope": "building"}, {"id": 31, "title": "notice-31", "body": "body-31", "from": "operator", "created_at": 1763100000031, "scope": "building"}, {"id": 32, "title": "notice-32", "body": "body-32", "from": "operator", "created_at": 1763100000032, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:407',407000,1794680184885,NULL,NULL,1794680184891,NULL,NULL,'turn-8',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:408','sb-dev:408','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',7038,'https://loca.example','sb-dev','reviewer','direct_user',1,1,408,408,'{"id": 408, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 33, "title": "notice-33", "body": "body-33", "from": "operator", "created_at": 1763100000033, "scope": "building"}, {"id": 34, "title": "notice-34", "body": "body-34", "from": "operator", "created_at": 1763100000034, "scope": "building"}, {"id": 35, "title": "notice-35", "body": "body-35", "from": "operator", "created_at": 1763100000035, "scope": "building"}, {"id": 36, "title": "notice-36", "body": "body-36", "from": "operator", "created_at": 1763100000036, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:408',408000,1794680184906,NULL,NULL,1794680184913,NULL,NULL,'turn-9',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:409','sb-dev:409','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',7921,'https://loca.example','sb-dev','reviewer','direct_user',1,1,409,409,'{"id": 409, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 37, "title": "notice-37", "body": "body-37", "from": "operator", "created_at": 1763100000037, "scope": "building"}, {"id": 38, "title": "notice-38", "body": "body-38", "from": "operator", "created_at": 1763100000038, "scope": "building"}, {"id": 39, "title": "notice-39", "body": "body-39", "from": "operator", "created_at": 1763100000039, "scope": "building"}, {"id": 40, "title": "notice-40", "body": "body-40", "from": "operator", "created_at": 1763100000040, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:409',409000,1794680184927,NULL,NULL,1794680184929,NULL,NULL,'turn-10',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:410','sb-dev:410','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',8804,'https://loca.example','sb-dev','reviewer','direct_user',1,1,410,410,'{"id": 410, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 41, "title": "notice-41", "body": "body-41", "from": "operator", "created_at": 1763100000041, "scope": "building"}, {"id": 42, "title": "notice-42", "body": "body-42", "from": "operator", "created_at": 1763100000042, "scope": "building"}, {"id": 43, "title": "notice-43", "body": "body-43", "from": "operator", "created_at": 1763100000043, "scope": "building"}, {"id": 44, "title": "notice-44", "body": "body-44", "from": "operator", "created_at": 1763100000044, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:410',410000,1794680184974,NULL,NULL,1794680184976,NULL,NULL,'turn-11',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:411','sb-dev:411','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',9687,'https://loca.example','sb-dev','reviewer','direct_user',1,1,411,411,'{"id": 411, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 45, "title": "notice-45", "body": "body-45", "from": "operator", "created_at": 1763100000045, "scope": "building"}, {"id": 46, "title": "notice-46", "body": "body-46", "from": "operator", "created_at": 1763100000046, "scope": "building"}, {"id": 47, "title": "notice-47", "body": "body-47", "from": "operator", "created_at": 1763100000047, "scope": "building"}, {"id": 48, "title": "notice-48", "body": "body-48", "from": "operator", "created_at": 1763100000048, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:411',411000,1794680185001,NULL,NULL,1794680185004,NULL,NULL,'turn-12',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:412','sb-dev:412','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',10570,'https://loca.example','sb-dev','reviewer','direct_user',1,1,412,412,'{"id": 412, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 49, "title": "notice-49", "body": "body-49", "from": "operator", "created_at": 1763100000049, "scope": "building"}, {"id": 50, "title": "notice-50", "body": "body-50", "from": "operator", "created_at": 1763100000050, "scope": "building"}, {"id": 51, "title": "notice-51", "body": "body-51", "from": "operator", "created_at": 1763100000051, "scope": "building"}, {"id": 52, "title": "notice-52", "body": "body-52", "from": "operator", "created_at": 1763100000052, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:412',412000,1794680185021,NULL,NULL,1794680185023,NULL,NULL,'turn-13',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:413','sb-dev:413','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',11453,'https://loca.example','sb-dev','reviewer','direct_user',1,1,413,413,'{"id": 413, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 53, "title": "notice-53", "body": "body-53", "from": "operator", "created_at": 1763100000053, "scope": "building"}, {"id": 54, "title": "notice-54", "body": "body-54", "from": "operator", "created_at": 1763100000054, "scope": "building"}, {"id": 55, "title": "notice-55", "body": "body-55", "from": "operator", "created_at": 1763100000055, "scope": "building"}, {"id": 56, "title": "notice-56", "body": "body-56", "from": "operator", "created_at": 1763100000056, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:413',413000,1794680185036,NULL,NULL,1794680185042,NULL,NULL,'turn-14',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:414','sb-dev:414','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',12336,'https://loca.example','sb-dev','reviewer','direct_user',1,1,414,414,'{"id": 414, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 57, "title": "notice-57", "body": "body-57", "from": "operator", "created_at": 1763100000057, "scope": "building"}, {"id": 58, "title": "notice-58", "body": "body-58", "from": "operator", "created_at": 1763100000058, "scope": "building"}, {"id": 59, "title": "notice-59", "body": "body-59", "from": "operator", "created_at": 1763100000059, "scope": "building"}, {"id": 60, "title": "notice-60", "body": "body-60", "from": "operator", "created_at": 1763100000060, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:414',414000,1794680185056,NULL,NULL,1794680185057,NULL,NULL,'turn-15',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:415','sb-dev:415','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',13219,'https://loca.example','sb-dev','reviewer','direct_user',1,1,415,415,'{"id": 415, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 61, "title": "notice-61", "body": "body-61", "from": "operator", "created_at": 1763100000061, "scope": "building"}, {"id": 62, "title": "notice-62", "body": "body-62", "from": "operator", "created_at": 1763100000062, "scope": "building"}, {"id": 63, "title": "notice-63", "body": "body-63", "from": "operator", "created_at": 1763100000063, "scope": "building"}, {"id": 64, "title": "notice-64", "body": "body-64", "from": "operator", "created_at": 1763100000064, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:415',415000,1794680185077,NULL,NULL,1794680185079,NULL,NULL,'turn-16',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:416','sb-dev:416','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',14102,'https://loca.example','sb-dev','reviewer','direct_user',1,1,416,416,'{"id": 416, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 65, "title": "notice-65", "body": "body-65", "from": "operator", "created_at": 1763100000065, "scope": "building"}, {"id": 66, "title": "notice-66", "body": "body-66", "from": "operator", "created_at": 1763100000066, "scope": "building"}, {"id": 67, "title": "notice-67", "body": "body-67", "from": "operator", "created_at": 1763100000067, "scope": "building"}, {"id": 68, "title": "notice-68", "body": "body-68", "from": "operator", "created_at": 1763100000068, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:416',416000,1794680185100,NULL,NULL,1794680185102,NULL,NULL,'turn-17',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:417','sb-dev:417','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',14985,'https://loca.example','sb-dev','reviewer','direct_user',1,1,417,417,'{"id": 417, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 69, "title": "notice-69", "body": "body-69", "from": "operator", "created_at": 1763100000069, "scope": "building"}, {"id": 70, "title": "notice-70", "body": "body-70", "from": "operator", "created_at": 1763100000070, "scope": "building"}, {"id": 71, "title": "notice-71", "body": "body-71", "from": "operator", "created_at": 1763100000071, "scope": "building"}, {"id": 72, "title": "notice-72", "body": "body-72", "from": "operator", "created_at": 1763100000072, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:417',417000,1794680185119,NULL,NULL,1794680185126,NULL,NULL,'turn-18',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:418','sb-dev:418','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',15868,'https://loca.example','sb-dev','reviewer','direct_user',1,1,418,418,'{"id": 418, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 73, "title": "notice-73", "body": "body-73", "from": "operator", "created_at": 1763100000073, "scope": "building"}, {"id": 74, "title": "notice-74", "body": "body-74", "from": "operator", "created_at": 1763100000074, "scope": "building"}, {"id": 75, "title": "notice-75", "body": "body-75", "from": "operator", "created_at": 1763100000075, "scope": "building"}, {"id": 76, "title": "notice-76", "body": "body-76", "from": "operator", "created_at": 1763100000076, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:418',418000,1794680185143,NULL,NULL,1794680185145,NULL,NULL,'turn-19',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:419','sb-dev:419','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',16751,'https://loca.example','sb-dev','reviewer','direct_user',1,1,419,419,'{"id": 419, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 77, "title": "notice-77", "body": "body-77", "from": "operator", "created_at": 1763100000077, "scope": "building"}, {"id": 78, "title": "notice-78", "body": "body-78", "from": "operator", "created_at": 1763100000078, "scope": "building"}, {"id": 79, "title": "notice-79", "body": "body-79", "from": "operator", "created_at": 1763100000079, "scope": "building"}, {"id": 80, "title": "notice-80", "body": "body-80", "from": "operator", "created_at": 1763100000080, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:419',419000,1794680185158,NULL,NULL,1794680185162,NULL,NULL,'turn-20',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:420','sb-dev:420','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',17634,'https://loca.example','sb-dev','reviewer','direct_user',1,1,420,420,'{"id": 420, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 81, "title": "notice-81", "body": "body-81", "from": "operator", "created_at": 1763100000081, "scope": "building"}, {"id": 82, "title": "notice-82", "body": "body-82", "from": "operator", "created_at": 1763100000082, "scope": "building"}, {"id": 83, "title": "notice-83", "body": "body-83", "from": "operator", "created_at": 1763100000083, "scope": "building"}, {"id": 84, "title": "notice-84", "body": "body-84", "from": "operator", "created_at": 1763100000084, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:420',420000,1794680185180,NULL,NULL,1794680185187,NULL,NULL,'turn-21',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:421','sb-dev:421','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',18517,'https://loca.example','sb-dev','reviewer','direct_user',1,1,421,421,'{"id": 421, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 85, "title": "notice-85", "body": "body-85", "from": "operator", "created_at": 1763100000085, "scope": "building"}, {"id": 86, "title": "notice-86", "body": "body-86", "from": "operator", "created_at": 1763100000086, "scope": "building"}, {"id": 87, "title": "notice-87", "body": "body-87", "from": "operator", "created_at": 1763100000087, "scope": "building"}, {"id": 88, "title": "notice-88", "body": "body-88", "from": "operator", "created_at": 1763100000088, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:421',421000,1794680185208,NULL,NULL,1794680185210,NULL,NULL,'turn-22',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:422','sb-dev:422','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',19400,'https://loca.example','sb-dev','reviewer','direct_user',1,1,422,422,'{"id": 422, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 89, "title": "notice-89", "body": "body-89", "from": "operator", "created_at": 1763100000089, "scope": "building"}, {"id": 90, "title": "notice-90", "body": "body-90", "from": "operator", "created_at": 1763100000090, "scope": "building"}, {"id": 91, "title": "notice-91", "body": "body-91", "from": "operator", "created_at": 1763100000091, "scope": "building"}, {"id": 92, "title": "notice-92", "body": "body-92", "from": "operator", "created_at": 1763100000092, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:422',422000,1794680185226,NULL,NULL,1794680185232,NULL,NULL,'turn-23',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:423','sb-dev:423','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',20283,'https://loca.example','sb-dev','reviewer','direct_user',1,1,423,423,'{"id": 423, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 93, "title": "notice-93", "body": "body-93", "from": "operator", "created_at": 1763100000093, "scope": "building"}, {"id": 94, "title": "notice-94", "body": "body-94", "from": "operator", "created_at": 1763100000094, "scope": "building"}, {"id": 95, "title": "notice-95", "body": "body-95", "from": "operator", "created_at": 1763100000095, "scope": "building"}, {"id": 96, "title": "notice-96", "body": "body-96", "from": "operator", "created_at": 1763100000096, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:423',423000,1794680185246,NULL,NULL,1794680185252,NULL,NULL,'turn-24',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:424','sb-dev:424','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',21166,'https://loca.example','sb-dev','reviewer','direct_user',1,1,424,424,'{"id": 424, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 97, "title": "notice-97", "body": "body-97", "from": "operator", "created_at": 1763100000097, "scope": "building"}, {"id": 98, "title": "notice-98", "body": "body-98", "from": "operator", "created_at": 1763100000098, "scope": "building"}, {"id": 99, "title": "notice-99", "body": "body-99", "from": "operator", "created_at": 1763100000099, "scope": "building"}, {"id": 100, "title": "notice-100", "body": "body-100", "from": "operator", "created_at": 1763100000100, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:424',424000,1794680185265,NULL,NULL,1794680185267,NULL,NULL,'turn-25',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:425','sb-dev:425','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',22052,'https://loca.example','sb-dev','reviewer','direct_user',1,1,425,425,'{"id": 425, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 101, "title": "notice-101", "body": "body-101", "from": "operator", "created_at": 1763100000101, "scope": "building"}, {"id": 102, "title": "notice-102", "body": "body-102", "from": "operator", "created_at": 1763100000102, "scope": "building"}, {"id": 103, "title": "notice-103", "body": "body-103", "from": "operator", "created_at": 1763100000103, "scope": "building"}, {"id": 104, "title": "notice-104", "body": "body-104", "from": "operator", "created_at": 1763100000104, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:425',425000,1794680185280,NULL,NULL,1794680185283,NULL,NULL,'turn-26',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:426','sb-dev:426','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',22947,'https://loca.example','sb-dev','reviewer','direct_user',1,1,426,426,'{"id": 426, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 105, "title": "notice-105", "body": "body-105", "from": "operator", "created_at": 1763100000105, "scope": "building"}, {"id": 106, "title": "notice-106", "body": "body-106", "from": "operator", "created_at": 1763100000106, "scope": "building"}, {"id": 107, "title": "notice-107", "body": "body-107", "from": "operator", "created_at": 1763100000107, "scope": "building"}, {"id": 108, "title": "notice-108", "body": "body-108", "from": "operator", "created_at": 1763100000108, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:426',426000,1794680185299,NULL,NULL,1794680185305,NULL,NULL,'turn-27',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:427','sb-dev:427','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',23842,'https://loca.example','sb-dev','reviewer','direct_user',1,1,427,427,'{"id": 427, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 109, "title": "notice-109", "body": "body-109", "from": "operator", "created_at": 1763100000109, "scope": "building"}, {"id": 110, "title": "notice-110", "body": "body-110", "from": "operator", "created_at": 1763100000110, "scope": "building"}, {"id": 111, "title": "notice-111", "body": "body-111", "from": "operator", "created_at": 1763100000111, "scope": "building"}, {"id": 112, "title": "notice-112", "body": "body-112", "from": "operator", "created_at": 1763100000112, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:427',427000,1794680185324,NULL,NULL,1794680185327,NULL,NULL,'turn-28',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:428','sb-dev:428','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',24737,'https://loca.example','sb-dev','reviewer','direct_user',1,1,428,428,'{"id": 428, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 113, "title": "notice-113", "body": "body-113", "from": "operator", "created_at": 1763100000113, "scope": "building"}, {"id": 114, "title": "notice-114", "body": "body-114", "from": "operator", "created_at": 1763100000114, "scope": "building"}, {"id": 115, "title": "notice-115", "body": "body-115", "from": "operator", "created_at": 1763100000115, "scope": "building"}, {"id": 116, "title": "notice-116", "body": "body-116", "from": "operator", "created_at": 1763100000116, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:428',428000,1794680185343,NULL,NULL,1794680185347,NULL,NULL,'turn-29',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:429','sb-dev:429','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',25632,'https://loca.example','sb-dev','reviewer','direct_user',1,1,429,429,'{"id": 429, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 117, "title": "notice-117", "body": "body-117", "from": "operator", "created_at": 1763100000117, "scope": "building"}, {"id": 118, "title": "notice-118", "body": "body-118", "from": "operator", "created_at": 1763100000118, "scope": "building"}, {"id": 119, "title": "notice-119", "body": "body-119", "from": "operator", "created_at": 1763100000119, "scope": "building"}, {"id": 120, "title": "notice-120", "body": "body-120", "from": "operator", "created_at": 1763100000120, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:429',429000,1794680185378,NULL,NULL,1794680185383,NULL,NULL,'turn-30',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:430','sb-dev:430','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',26527,'https://loca.example','sb-dev','reviewer','direct_user',1,1,430,430,'{"id": 430, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 121, "title": "notice-121", "body": "body-121", "from": "operator", "created_at": 1763100000121, "scope": "building"}, {"id": 122, "title": "notice-122", "body": "body-122", "from": "operator", "created_at": 1763100000122, "scope": "building"}, {"id": 123, "title": "notice-123", "body": "body-123", "from": "operator", "created_at": 1763100000123, "scope": "building"}, {"id": 124, "title": "notice-124", "body": "body-124", "from": "operator", "created_at": 1763100000124, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:430',430000,1794680185396,NULL,NULL,1794680185399,NULL,NULL,'turn-31',2,1,0);
INSERT INTO "attentions" VALUES('attention:reviewer:fc55b34570ce:sb-dev:431','sb-dev:431','reviewer:/tmp/tmpud1trkhg/inbox.jsonl',27422,'https://loca.example','sb-dev','reviewer','direct_user',1,1,431,431,'{"id": 431, "room": "sb-dev", "sender": "operator", "sender_type": "user", "target": "reviewer", "text": "ping", "notification_context": {"recipient": "reviewer", "items": [{"id": 125, "title": "notice-125", "body": "body-125", "from": "operator", "created_at": 1763100000125, "scope": "building"}, {"id": 126, "title": "notice-126", "body": "body-126", "from": "operator", "created_at": 1763100000126, "scope": "building"}, {"id": 127, "title": "notice-127", "body": "body-127", "from": "operator", "created_at": 1763100000127, "scope": "building"}, {"id": 128, "title": "notice-128", "body": "body-128", "from": "operator", "created_at": 1763100000128, "scope": "building"}], "more_available": true}}','bundle:attention:reviewer:fc55b34570ce:sb-dev:431',431000,1794680185418,NULL,NULL,1794680185420,NULL,NULL,'turn-32',2,1,0);
CREATE TABLE bundle_attentions (
                    bundle_id TEXT NOT NULL REFERENCES bundles(bundle_id),
                    attention_id TEXT NOT NULL REFERENCES attentions(attention_id),
                    position INTEGER NOT NULL,
                    PRIMARY KEY (bundle_id, attention_id),
                    UNIQUE (bundle_id, position)
                );
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:400','attention:reviewer:fc55b34570ce:sb-dev:400',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:401','attention:reviewer:fc55b34570ce:sb-dev:401',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:402','attention:reviewer:fc55b34570ce:sb-dev:402',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:403','attention:reviewer:fc55b34570ce:sb-dev:403',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:404','attention:reviewer:fc55b34570ce:sb-dev:404',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:405','attention:reviewer:fc55b34570ce:sb-dev:405',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:406','attention:reviewer:fc55b34570ce:sb-dev:406',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:407','attention:reviewer:fc55b34570ce:sb-dev:407',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:408','attention:reviewer:fc55b34570ce:sb-dev:408',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:409','attention:reviewer:fc55b34570ce:sb-dev:409',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:410','attention:reviewer:fc55b34570ce:sb-dev:410',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:411','attention:reviewer:fc55b34570ce:sb-dev:411',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:412','attention:reviewer:fc55b34570ce:sb-dev:412',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:413','attention:reviewer:fc55b34570ce:sb-dev:413',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:414','attention:reviewer:fc55b34570ce:sb-dev:414',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:415','attention:reviewer:fc55b34570ce:sb-dev:415',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:416','attention:reviewer:fc55b34570ce:sb-dev:416',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:417','attention:reviewer:fc55b34570ce:sb-dev:417',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:418','attention:reviewer:fc55b34570ce:sb-dev:418',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:419','attention:reviewer:fc55b34570ce:sb-dev:419',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:420','attention:reviewer:fc55b34570ce:sb-dev:420',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:421','attention:reviewer:fc55b34570ce:sb-dev:421',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:422','attention:reviewer:fc55b34570ce:sb-dev:422',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:423','attention:reviewer:fc55b34570ce:sb-dev:423',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:424','attention:reviewer:fc55b34570ce:sb-dev:424',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:425','attention:reviewer:fc55b34570ce:sb-dev:425',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:426','attention:reviewer:fc55b34570ce:sb-dev:426',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:427','attention:reviewer:fc55b34570ce:sb-dev:427',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:428','attention:reviewer:fc55b34570ce:sb-dev:428',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:429','attention:reviewer:fc55b34570ce:sb-dev:429',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:430','attention:reviewer:fc55b34570ce:sb-dev:430',0);
INSERT INTO "bundle_attentions" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:431','attention:reviewer:fc55b34570ce:sb-dev:431',0);
CREATE TABLE bundles (
                    bundle_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL
                );
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:400','reviewer','direct_user',1794680184730);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:401','reviewer','direct_user',1794680184752);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:402','reviewer','direct_user',1794680184779);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:403','reviewer','direct_user',1794680184797);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:404','reviewer','direct_user',1794680184813);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:405','reviewer','direct_user',1794680184832);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:406','reviewer','direct_user',1794680184855);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:407','reviewer','direct_user',1794680184870);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:408','reviewer','direct_user',1794680184903);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:409','reviewer','direct_user',1794680184922);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:410','reviewer','direct_user',1794680184941);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:411','reviewer','direct_user',1794680184997);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:412','reviewer','direct_user',1794680185013);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:413','reviewer','direct_user',1794680185032);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:414','reviewer','direct_user',1794680185046);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:415','reviewer','direct_user',1794680185067);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:416','reviewer','direct_user',1794680185093);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:417','reviewer','direct_user',1794680185112);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:418','reviewer','direct_user',1794680185133);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:419','reviewer','direct_user',1794680185154);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:420','reviewer','direct_user',1794680185171);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:421','reviewer','direct_user',1794680185197);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:422','reviewer','direct_user',1794680185219);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:423','reviewer','direct_user',1794680185239);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:424','reviewer','direct_user',1794680185258);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:425','reviewer','direct_user',1794680185275);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:426','reviewer','direct_user',1794680185290);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:427','reviewer','direct_user',1794680185312);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:428','reviewer','direct_user',1794680185335);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:429','reviewer','direct_user',1794680185365);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:430','reviewer','direct_user',1794680185390);
INSERT INTO "bundles" VALUES('bundle:attention:reviewer:fc55b34570ce:sb-dev:431','reviewer','direct_user',1794680185406);
CREATE TABLE context_watermarks (
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    last_id INTEGER NOT NULL,
                    observed_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (server, room)
                );
INSERT INTO "context_watermarks" VALUES('https://loca.example','sb-dev',431,1794680185406);
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
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:400','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:400','accepted','turn-1',1794680184741,1794680184742,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:401','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:401','accepted','turn-2',1794680184760,1794680184762,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:402','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:402','accepted','turn-3',1794680184781,1794680184787,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:403','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:403','accepted','turn-4',1794680184799,1794680184802,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:404','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:404','accepted','turn-5',1794680184820,1794680184821,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:405','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:405','accepted','turn-6',1794680184835,1794680184837,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:406','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:406','accepted','turn-7',1794680184858,1794680184859,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:407','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:407','accepted','turn-8',1794680184879,1794680184885,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:408','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:408','accepted','turn-9',1794680184905,1794680184906,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:409','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:409','accepted','turn-10',1794680184925,1794680184927,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:410','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:410','accepted','turn-11',1794680184967,1794680184974,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:411','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:411','accepted','turn-12',1794680185000,1794680185001,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:412','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:412','accepted','turn-13',1794680185020,1794680185021,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:413','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:413','accepted','turn-14',1794680185035,1794680185036,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:414','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:414','accepted','turn-15',1794680185054,1794680185056,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:415','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:415','accepted','turn-16',1794680185070,1794680185077,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:416','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:416','accepted','turn-17',1794680185096,1794680185100,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:417','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:417','accepted','turn-18',1794680185118,1794680185119,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:418','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:418','accepted','turn-19',1794680185142,1794680185143,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:419','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:419','accepted','turn-20',1794680185157,1794680185158,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:420','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:420','accepted','turn-21',1794680185179,1794680185180,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:421','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:421','accepted','turn-22',1794680185203,1794680185208,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:422','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:422','accepted','turn-23',1794680185225,1794680185226,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:423','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:423','accepted','turn-24',1794680185244,1794680185246,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:424','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:424','accepted','turn-25',1794680185264,1794680185265,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:425','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:425','accepted','turn-26',1794680185279,1794680185280,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:426','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:426','accepted','turn-27',1794680185298,1794680185299,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:427','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:427','accepted','turn-28',1794680185316,1794680185324,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:428','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:428','accepted','turn-29',1794680185342,1794680185343,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:429','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:429','accepted','turn-30',1794680185373,1794680185378,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:430','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:430','accepted','turn-31',1794680185395,1794680185396,NULL,2);
INSERT INTO "dispatch_intents" VALUES('attention:reviewer:fc55b34570ce:sb-dev:431','reviewer','https://loca.example','sb-dev','thread-1','start','loca:attention:reviewer:fc55b34570ce:sb-dev:431','accepted','turn-32',1794680185410,1794680185418,NULL,2);
CREATE TABLE ingestion_sources (
                    source_id TEXT PRIMARY KEY,
                    path TEXT NOT NULL,
                    offset INTEGER NOT NULL DEFAULT 0,
                    updated_at_ms INTEGER NOT NULL
                );
INSERT INTO "ingestion_sources" VALUES('reviewer:/tmp/tmpud1trkhg/inbox.jsonl','/tmp/tmpud1trkhg/inbox.jsonl',28317,1794680185406);
CREATE TABLE leases (
                    identity TEXT PRIMARY KEY,
                    lease_owner TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    lease_expires_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );
INSERT INTO "leases" VALUES('reviewer','3520914-f51fd8629c2443c7887b73700f3590f1',2,1794680185421,1794680185421);
CREATE TABLE notification_context (
                    identity TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    notification_id INTEGER NOT NULL,
                    turn_id TEXT NOT NULL,
                    accepted_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (identity, thread_id, notification_id)
                );
INSERT INTO "notification_context" VALUES('reviewer','thread-1',1,'turn-1',1794680184742);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',2,'turn-1',1794680184742);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',3,'turn-1',1794680184742);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',4,'turn-1',1794680184742);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',5,'turn-2',1794680184762);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',6,'turn-2',1794680184762);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',7,'turn-2',1794680184762);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',8,'turn-2',1794680184762);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',9,'turn-3',1794680184787);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',10,'turn-3',1794680184787);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',11,'turn-3',1794680184787);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',12,'turn-3',1794680184787);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',13,'turn-4',1794680184802);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',14,'turn-4',1794680184802);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',15,'turn-4',1794680184802);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',16,'turn-4',1794680184802);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',17,'turn-5',1794680184821);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',18,'turn-5',1794680184821);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',19,'turn-5',1794680184821);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',20,'turn-5',1794680184821);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',21,'turn-6',1794680184837);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',22,'turn-6',1794680184837);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',23,'turn-6',1794680184837);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',24,'turn-6',1794680184837);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',25,'turn-7',1794680184859);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',26,'turn-7',1794680184859);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',27,'turn-7',1794680184859);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',28,'turn-7',1794680184859);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',29,'turn-8',1794680184885);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',30,'turn-8',1794680184885);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',31,'turn-8',1794680184885);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',32,'turn-8',1794680184885);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',33,'turn-9',1794680184906);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',34,'turn-9',1794680184906);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',35,'turn-9',1794680184906);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',36,'turn-9',1794680184906);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',37,'turn-10',1794680184927);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',38,'turn-10',1794680184927);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',39,'turn-10',1794680184927);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',40,'turn-10',1794680184927);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',41,'turn-11',1794680184974);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',42,'turn-11',1794680184974);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',43,'turn-11',1794680184974);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',44,'turn-11',1794680184974);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',45,'turn-12',1794680185001);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',46,'turn-12',1794680185001);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',47,'turn-12',1794680185001);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',48,'turn-12',1794680185001);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',49,'turn-13',1794680185021);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',50,'turn-13',1794680185021);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',51,'turn-13',1794680185021);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',52,'turn-13',1794680185021);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',53,'turn-14',1794680185036);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',54,'turn-14',1794680185036);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',55,'turn-14',1794680185036);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',56,'turn-14',1794680185036);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',57,'turn-15',1794680185056);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',58,'turn-15',1794680185056);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',59,'turn-15',1794680185056);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',60,'turn-15',1794680185056);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',61,'turn-16',1794680185077);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',62,'turn-16',1794680185077);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',63,'turn-16',1794680185077);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',64,'turn-16',1794680185077);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',65,'turn-17',1794680185100);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',66,'turn-17',1794680185100);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',67,'turn-17',1794680185100);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',68,'turn-17',1794680185100);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',69,'turn-18',1794680185119);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',70,'turn-18',1794680185119);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',71,'turn-18',1794680185119);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',72,'turn-18',1794680185119);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',73,'turn-19',1794680185143);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',74,'turn-19',1794680185143);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',75,'turn-19',1794680185143);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',76,'turn-19',1794680185143);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',77,'turn-20',1794680185158);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',78,'turn-20',1794680185158);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',79,'turn-20',1794680185158);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',80,'turn-20',1794680185158);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',81,'turn-21',1794680185180);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',82,'turn-21',1794680185180);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',83,'turn-21',1794680185180);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',84,'turn-21',1794680185180);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',85,'turn-22',1794680185208);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',86,'turn-22',1794680185208);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',87,'turn-22',1794680185208);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',88,'turn-22',1794680185208);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',89,'turn-23',1794680185226);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',90,'turn-23',1794680185226);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',91,'turn-23',1794680185226);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',92,'turn-23',1794680185226);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',93,'turn-24',1794680185246);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',94,'turn-24',1794680185246);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',95,'turn-24',1794680185246);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',96,'turn-24',1794680185246);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',97,'turn-25',1794680185265);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',98,'turn-25',1794680185265);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',99,'turn-25',1794680185265);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',100,'turn-25',1794680185265);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',101,'turn-26',1794680185280);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',102,'turn-26',1794680185280);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',103,'turn-26',1794680185280);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',104,'turn-26',1794680185280);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',105,'turn-27',1794680185299);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',106,'turn-27',1794680185299);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',107,'turn-27',1794680185299);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',108,'turn-27',1794680185299);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',109,'turn-28',1794680185324);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',110,'turn-28',1794680185324);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',111,'turn-28',1794680185324);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',112,'turn-28',1794680185324);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',113,'turn-29',1794680185343);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',114,'turn-29',1794680185343);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',115,'turn-29',1794680185343);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',116,'turn-29',1794680185343);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',117,'turn-30',1794680185378);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',118,'turn-30',1794680185378);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',119,'turn-30',1794680185378);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',120,'turn-30',1794680185378);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',121,'turn-31',1794680185396);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',122,'turn-31',1794680185396);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',123,'turn-31',1794680185396);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',124,'turn-31',1794680185396);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',125,'turn-32',1794680185418);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',126,'turn-32',1794680185418);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',127,'turn-32',1794680185418);
INSERT INTO "notification_context" VALUES('reviewer','thread-1',128,'turn-32',1794680185418);
CREATE TABLE notification_threads (
                    identity TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    floor_id INTEGER NOT NULL,
                    PRIMARY KEY (identity, thread_id)
                );
INSERT INTO "notification_threads" VALUES('reviewer','thread-1',1);
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
INSERT INTO "threads" VALUES('https://loca.example','sb-dev','reviewer','thread-1',2,1794680184739,1794680184739);
CREATE TABLE turn_attentions (
                    turn_id TEXT NOT NULL REFERENCES turns(turn_id),
                    attention_id TEXT NOT NULL REFERENCES attentions(attention_id),
                    position INTEGER NOT NULL,
                    PRIMARY KEY (turn_id, attention_id),
                    UNIQUE (turn_id, position)
                );
INSERT INTO "turn_attentions" VALUES('turn-1','attention:reviewer:fc55b34570ce:sb-dev:400',0);
INSERT INTO "turn_attentions" VALUES('turn-2','attention:reviewer:fc55b34570ce:sb-dev:401',0);
INSERT INTO "turn_attentions" VALUES('turn-3','attention:reviewer:fc55b34570ce:sb-dev:402',0);
INSERT INTO "turn_attentions" VALUES('turn-4','attention:reviewer:fc55b34570ce:sb-dev:403',0);
INSERT INTO "turn_attentions" VALUES('turn-5','attention:reviewer:fc55b34570ce:sb-dev:404',0);
INSERT INTO "turn_attentions" VALUES('turn-6','attention:reviewer:fc55b34570ce:sb-dev:405',0);
INSERT INTO "turn_attentions" VALUES('turn-7','attention:reviewer:fc55b34570ce:sb-dev:406',0);
INSERT INTO "turn_attentions" VALUES('turn-8','attention:reviewer:fc55b34570ce:sb-dev:407',0);
INSERT INTO "turn_attentions" VALUES('turn-9','attention:reviewer:fc55b34570ce:sb-dev:408',0);
INSERT INTO "turn_attentions" VALUES('turn-10','attention:reviewer:fc55b34570ce:sb-dev:409',0);
INSERT INTO "turn_attentions" VALUES('turn-11','attention:reviewer:fc55b34570ce:sb-dev:410',0);
INSERT INTO "turn_attentions" VALUES('turn-12','attention:reviewer:fc55b34570ce:sb-dev:411',0);
INSERT INTO "turn_attentions" VALUES('turn-13','attention:reviewer:fc55b34570ce:sb-dev:412',0);
INSERT INTO "turn_attentions" VALUES('turn-14','attention:reviewer:fc55b34570ce:sb-dev:413',0);
INSERT INTO "turn_attentions" VALUES('turn-15','attention:reviewer:fc55b34570ce:sb-dev:414',0);
INSERT INTO "turn_attentions" VALUES('turn-16','attention:reviewer:fc55b34570ce:sb-dev:415',0);
INSERT INTO "turn_attentions" VALUES('turn-17','attention:reviewer:fc55b34570ce:sb-dev:416',0);
INSERT INTO "turn_attentions" VALUES('turn-18','attention:reviewer:fc55b34570ce:sb-dev:417',0);
INSERT INTO "turn_attentions" VALUES('turn-19','attention:reviewer:fc55b34570ce:sb-dev:418',0);
INSERT INTO "turn_attentions" VALUES('turn-20','attention:reviewer:fc55b34570ce:sb-dev:419',0);
INSERT INTO "turn_attentions" VALUES('turn-21','attention:reviewer:fc55b34570ce:sb-dev:420',0);
INSERT INTO "turn_attentions" VALUES('turn-22','attention:reviewer:fc55b34570ce:sb-dev:421',0);
INSERT INTO "turn_attentions" VALUES('turn-23','attention:reviewer:fc55b34570ce:sb-dev:422',0);
INSERT INTO "turn_attentions" VALUES('turn-24','attention:reviewer:fc55b34570ce:sb-dev:423',0);
INSERT INTO "turn_attentions" VALUES('turn-25','attention:reviewer:fc55b34570ce:sb-dev:424',0);
INSERT INTO "turn_attentions" VALUES('turn-26','attention:reviewer:fc55b34570ce:sb-dev:425',0);
INSERT INTO "turn_attentions" VALUES('turn-27','attention:reviewer:fc55b34570ce:sb-dev:426',0);
INSERT INTO "turn_attentions" VALUES('turn-28','attention:reviewer:fc55b34570ce:sb-dev:427',0);
INSERT INTO "turn_attentions" VALUES('turn-29','attention:reviewer:fc55b34570ce:sb-dev:428',0);
INSERT INTO "turn_attentions" VALUES('turn-30','attention:reviewer:fc55b34570ce:sb-dev:429',0);
INSERT INTO "turn_attentions" VALUES('turn-31','attention:reviewer:fc55b34570ce:sb-dev:430',0);
INSERT INTO "turn_attentions" VALUES('turn-32','attention:reviewer:fc55b34570ce:sb-dev:431',0);
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
INSERT INTO "turns" VALUES('turn-1','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184742,1794680184749,NULL,NULL);
INSERT INTO "turns" VALUES('turn-2','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184762,1794680184768,NULL,NULL);
INSERT INTO "turns" VALUES('turn-3','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184787,1794680184788,NULL,NULL);
INSERT INTO "turns" VALUES('turn-4','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184802,1794680184803,NULL,NULL);
INSERT INTO "turns" VALUES('turn-5','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184821,1794680184823,NULL,NULL);
INSERT INTO "turns" VALUES('turn-6','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184837,1794680184844,NULL,NULL);
INSERT INTO "turns" VALUES('turn-7','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184859,1794680184866,NULL,NULL);
INSERT INTO "turns" VALUES('turn-8','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184885,1794680184891,NULL,NULL);
INSERT INTO "turns" VALUES('turn-9','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184906,1794680184913,NULL,NULL);
INSERT INTO "turns" VALUES('turn-10','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184927,1794680184929,NULL,NULL);
INSERT INTO "turns" VALUES('turn-11','reviewer','https://loca.example','sb-dev','thread-1',2,1794680184974,1794680184976,NULL,NULL);
INSERT INTO "turns" VALUES('turn-12','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185001,1794680185004,NULL,NULL);
INSERT INTO "turns" VALUES('turn-13','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185021,1794680185023,NULL,NULL);
INSERT INTO "turns" VALUES('turn-14','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185036,1794680185042,NULL,NULL);
INSERT INTO "turns" VALUES('turn-15','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185056,1794680185057,NULL,NULL);
INSERT INTO "turns" VALUES('turn-16','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185077,1794680185079,NULL,NULL);
INSERT INTO "turns" VALUES('turn-17','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185100,1794680185102,NULL,NULL);
INSERT INTO "turns" VALUES('turn-18','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185119,1794680185126,NULL,NULL);
INSERT INTO "turns" VALUES('turn-19','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185143,1794680185145,NULL,NULL);
INSERT INTO "turns" VALUES('turn-20','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185158,1794680185162,NULL,NULL);
INSERT INTO "turns" VALUES('turn-21','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185180,1794680185187,NULL,NULL);
INSERT INTO "turns" VALUES('turn-22','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185208,1794680185210,NULL,NULL);
INSERT INTO "turns" VALUES('turn-23','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185226,1794680185232,NULL,NULL);
INSERT INTO "turns" VALUES('turn-24','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185246,1794680185252,NULL,NULL);
INSERT INTO "turns" VALUES('turn-25','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185265,1794680185267,NULL,NULL);
INSERT INTO "turns" VALUES('turn-26','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185280,1794680185283,NULL,NULL);
INSERT INTO "turns" VALUES('turn-27','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185299,1794680185305,NULL,NULL);
INSERT INTO "turns" VALUES('turn-28','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185324,1794680185327,NULL,NULL);
INSERT INTO "turns" VALUES('turn-29','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185343,1794680185347,NULL,NULL);
INSERT INTO "turns" VALUES('turn-30','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185378,1794680185383,NULL,NULL);
INSERT INTO "turns" VALUES('turn-31','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185396,1794680185399,NULL,NULL);
INSERT INTO "turns" VALUES('turn-32','reviewer','https://loca.example','sb-dev','thread-1',2,1794680185418,1794680185420,NULL,NULL);
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
