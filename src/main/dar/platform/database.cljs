(ns dar.platform.database
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            ["node:sqlite" :refer [DatabaseSync]]))

(def migrations
  [{:version 1
    :sql "
CREATE TABLE users (
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  email TEXT,
  administrator INTEGER NOT NULL DEFAULT 0 CHECK (administrator IN (0, 1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
  id_hash BLOB PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  csrf_hash BLOB NOT NULL,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
) STRICT;

CREATE TABLE app_passwords (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  label TEXT NOT NULL,
  secret_hash BLOB NOT NULL,
  scopes_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
) STRICT;

CREATE TABLE preferences (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  namespace TEXT NOT NULL,
  key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, namespace, key)
) STRICT;

CREATE TABLE mutation_journal (
  id TEXT PRIMARY KEY,
  user_id TEXT,
  operation TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('prepared', 'committed', 'reconciled')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE search_documents (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  document_id TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  url TEXT NOT NULL,
  generation INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, provider, document_id)
) STRICT;

CREATE INDEX search_documents_user_provider
  ON search_documents(user_id, provider, updated_at DESC);

CREATE TABLE activity_events (
  id TEXT PRIMARY KEY,
  actor_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
  app_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  subject_json TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;

CREATE TABLE activity_recipients (
  event_id TEXT NOT NULL REFERENCES activity_events(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (event_id, user_id)
) STRICT;

CREATE TABLE activity_outbox (
  event_id TEXT PRIMARY KEY REFERENCES activity_events(id) ON DELETE CASCADE,
  attempts INTEGER NOT NULL DEFAULT 0,
  available_at TEXT NOT NULL,
  last_error TEXT
) STRICT;

CREATE TABLE notifications (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  dedupe_key TEXT,
  app_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  read_at TEXT,
  created_at TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX notifications_user_dedupe
  ON notifications(user_id, dedupe_key) WHERE dedupe_key IS NOT NULL;

CREATE TABLE widget_layouts (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  widget_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  size TEXT NOT NULL,
  settings_json TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (user_id, widget_id)
) STRICT;

CREATE TABLE plugins (
  id TEXT PRIMARY KEY,
  repository_url TEXT NOT NULL,
  active_commit TEXT,
  previous_commit TEXT,
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
  manifest_json TEXT,
  error TEXT,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE plugin_migrations (
  plugin_id TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  commit_hash TEXT NOT NULL,
  version INTEGER NOT NULL,
  applied_at TEXT NOT NULL,
  PRIMARY KEY (plugin_id, version)
) STRICT;"}
   {:version 2
    :sql "ALTER TABLE app_passwords ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE app_passwords ADD COLUMN prefix TEXT;"}
   {:version 3
    :sql "
-- Files metadata. Portable file bytes live under each user's files/ root;
-- these tables only index them. File rows are path-keyed, records that
-- survive renames (trash, versions, shares) are id-keyed, per the rewrite
-- binding contract in docs/rewrite/storage-and-lifecycle.md.
CREATE TABLE files_favorites (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  PRIMARY KEY (user_id, path)
) STRICT;

CREATE TABLE files_tags (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  color TEXT NOT NULL,
  PRIMARY KEY (user_id, id)
) STRICT;

CREATE TABLE files_file_tags (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  tag_id TEXT NOT NULL,
  PRIMARY KEY (user_id, path, tag_id)
) STRICT;

CREATE TABLE files_trash (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  original_path TEXT NOT NULL,
  trash_path TEXT NOT NULL,
  deleted_at TEXT NOT NULL,
  PRIMARY KEY (user_id, id)
) STRICT;

CREATE TABLE files_versions (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  id TEXT NOT NULL,
  blob_path TEXT NOT NULL,
  size INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (user_id, path, id)
) STRICT;

CREATE TABLE files_public_shares (
  token TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  password_salt BLOB NOT NULL DEFAULT X'',
  password_hash BLOB NOT NULL DEFAULT X'',
  expires_at TEXT NOT NULL,
  drop_only INTEGER NOT NULL DEFAULT 0 CHECK (drop_only IN (0, 1)),
  created_at TEXT NOT NULL
) STRICT;

CREATE INDEX files_public_shares_user
  ON files_public_shares(user_id, created_at DESC);

CREATE TABLE files_user_shares (
  id TEXT PRIMARY KEY,
  from_user TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  to_user TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  permission TEXT NOT NULL CHECK (permission IN ('read', 'write')),
  created_at TEXT NOT NULL
) STRICT;

CREATE INDEX files_user_shares_users
  ON files_user_shares(from_user, to_user);

CREATE TABLE files_comments (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  id TEXT NOT NULL,
  author TEXT NOT NULL,
  body TEXT NOT NULL,
  parent_id TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  PRIMARY KEY (user_id, path, id)
) STRICT;

CREATE TABLE files_locks (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  owner TEXT NOT NULL,
  PRIMARY KEY (user_id, path)
) STRICT;

CREATE TABLE files_activity (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  action TEXT NOT NULL,
  happened_at TEXT NOT NULL,
  PRIMARY KEY (user_id, id)
) STRICT;

CREATE INDEX files_activity_recent
  ON files_activity(user_id, happened_at DESC);

CREATE TABLE files_operation_leases (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  token TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  PRIMARY KEY (user_id, path)
) STRICT;"}
   {:version 4
    :sql "
-- Provider-level generations make empty and failed search reconciliations
-- monotonic without exposing a partially replaced document set.
CREATE TABLE search_provider_generations (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  generation INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, provider)
) STRICT;

-- Locks are renewable capabilities rather than permanent owner labels. Empty
-- defaults only serve fresh migration compatibility; the Files service replaces
-- them on the first successful lock acquisition.
ALTER TABLE files_locks ADD COLUMN token TEXT NOT NULL DEFAULT '';
ALTER TABLE files_locks ADD COLUMN expires_at TEXT NOT NULL DEFAULT '';
"}

 ;; Calendar keeps portable ICS files as the authority; these tables are the
 ;; derived metadata and occurrence index per
 ;; docs/rewrite/storage-and-lifecycle.md.
  {:version 5
   :sql "
CREATE TABLE calendar_metadata (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  calendar_id TEXT NOT NULL,
  name TEXT NOT NULL,
  color TEXT NOT NULL DEFAULT '',
  visible INTEGER NOT NULL DEFAULT 1,
  sort_order INTEGER NOT NULL DEFAULT 0,
  kind TEXT NOT NULL DEFAULT 'local',
  subscription_url TEXT NOT NULL DEFAULT '',
  last_synced_at TEXT,
  sync_error TEXT,
  revision INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, calendar_id)
) STRICT;

CREATE TABLE calendar_events (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  calendar_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  master_id TEXT NOT NULL DEFAULT '',
  recurrence_id INTEGER,
  title TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  location TEXT NOT NULL DEFAULT '',
  start_ts INTEGER NOT NULL,
  end_ts INTEGER NOT NULL,
  all_day INTEGER NOT NULL DEFAULT 0,
  timezone TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT '',
  busy INTEGER NOT NULL DEFAULT 0,
  color TEXT NOT NULL DEFAULT '',
  rrule TEXT NOT NULL DEFAULT '',
  exdates TEXT NOT NULL DEFAULT '[]',
  reminders TEXT NOT NULL DEFAULT '[]',
  revision INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, calendar_id, event_id)
) STRICT;

CREATE INDEX calendar_events_range
  ON calendar_events(user_id, start_ts, end_ts);

CREATE INDEX calendar_events_search
  ON calendar_events(user_id, title);
"}
   {:version 6
    :sql "CREATE TABLE contacts_revisions (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  resource TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (user_id, resource)
) STRICT;"}
   {:version 7
    :sql "CREATE TABLE personal_profiles (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  values_json TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  me_contact_id TEXT NOT NULL,
  synced_revision INTEGER NOT NULL DEFAULT 0,
  sync_error TEXT,
  updated_at TEXT NOT NULL
) STRICT;"}
   {:version 8
    :sql "CREATE TABLE activity_feed_settings (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  enabled INTEGER NOT NULL DEFAULT 0,
  sources_json TEXT NOT NULL,
  token_hash TEXT UNIQUE,
  revision INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
) STRICT;"}
   {:version 9
    :sql "CREATE TABLE chat_conversations (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  harness TEXT NOT NULL DEFAULT 'codex',
  model TEXT NOT NULL DEFAULT '',
  reasoning_effort TEXT NOT NULL DEFAULT '',
  archived INTEGER NOT NULL DEFAULT 0,
  draft TEXT NOT NULL DEFAULT '',
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;
CREATE INDEX chat_conversations_user ON chat_conversations(user_id, archived, updated_at DESC, id DESC);"}
   {:version 10
    :sql "CREATE TABLE chat_credentials (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  encrypted_value TEXT,
  revision INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
) STRICT;"}
   {:version 11
    :sql "ALTER TABLE chat_conversations ADD COLUMN native_thread_id TEXT;
CREATE TABLE chat_turns (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  conversation_id TEXT NOT NULL REFERENCES chat_conversations(id) ON DELETE CASCADE,
  request_id TEXT NOT NULL,
  input TEXT NOT NULL,
  output TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'starting',
  model TEXT NOT NULL,
  reasoning_effort TEXT NOT NULL,
  native_turn_id TEXT,
  error TEXT,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(user_id, conversation_id, request_id)
) STRICT;
CREATE INDEX chat_turns_conversation ON chat_turns(user_id, conversation_id, created_at, id);
CREATE UNIQUE INDEX chat_turns_active ON chat_turns(conversation_id) WHERE status IN ('starting', 'running');"}
   {:version 12
    :sql "CREATE TABLE chat_tool_calls (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  conversation_id TEXT NOT NULL REFERENCES chat_conversations(id) ON DELETE CASCADE,
  turn_id TEXT NOT NULL REFERENCES chat_turns(id) ON DELETE CASCADE,
  call_id TEXT NOT NULL,
  tool TEXT NOT NULL,
  title TEXT NOT NULL,
  effect TEXT NOT NULL,
  arguments_json TEXT NOT NULL,
  result_json TEXT,
  status TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  UNIQUE(turn_id, call_id)
) STRICT;"}
   {:version 13
    :sql "CREATE TABLE todo_lists (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  metadata_json TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1
) STRICT;
CREATE INDEX todo_lists_owner ON todo_lists(user_id, id);
CREATE TABLE todo_members (
  list_id TEXT NOT NULL REFERENCES todo_lists(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  permission TEXT NOT NULL CHECK(permission IN ('read', 'write')),
  PRIMARY KEY(list_id, user_id)
) STRICT;
CREATE TABLE todo_tasks (
  id TEXT PRIMARY KEY,
  list_id TEXT NOT NULL REFERENCES todo_lists(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  metadata_json TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1
) STRICT;
CREATE INDEX todo_tasks_list ON todo_tasks(list_id, id);"}
   {:version 14
    :sql "CREATE TABLE notes_index (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  metadata_json TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT
) STRICT;
CREATE INDEX notes_index_owner ON notes_index(user_id, id);"}
   {:version 15
    :sql "CREATE TABLE todo_generated_lists (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  list_id TEXT NOT NULL,
  PRIMARY KEY(user_id, source)
) STRICT;
CREATE TABLE todo_generated_tasks (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  external_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  created INTEGER NOT NULL DEFAULT 0,
  retired INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(user_id, source, external_id)
) STRICT;"}
   {:version 16
    :sql "CREATE TABLE prayer_occurrences (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  local_date TEXT NOT NULL,
  prayer TEXT NOT NULL,
  external_id TEXT NOT NULL,
  scheduled_at TEXT NOT NULL,
  window_end_at TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  UNIQUE(user_id, local_date, prayer)
) STRICT;
CREATE TABLE prayer_sync (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  last_synced_at TEXT,
  last_error TEXT
) STRICT;"}
   {:version 17
    :sql "CREATE TABLE weather_cache (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  location_key TEXT NOT NULL,
  payload_json TEXT,
  fetched_at TEXT,
  retry_at TEXT NOT NULL,
  last_error TEXT
) STRICT;"}
   {:version 18
    :sql "CREATE TABLE news_folders (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  UNIQUE(user_id, name)
) STRICT;
CREATE TABLE news_feeds (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  title TEXT NOT NULL,
  site_url TEXT,
  folder_id TEXT REFERENCES news_folders(id) ON DELETE SET NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  next_refresh_at TEXT NOT NULL,
  last_refreshed_at TEXT,
  last_error TEXT,
  error_count INTEGER NOT NULL DEFAULT 0,
  refresh_enabled INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  UNIQUE(user_id, url)
) STRICT;
CREATE INDEX news_feeds_due ON news_feeds(next_refresh_at) WHERE deleted_at IS NULL AND refresh_enabled=1;
CREATE TABLE news_items (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  feed_id TEXT NOT NULL REFERENCES news_feeds(id) ON DELETE CASCADE,
  external_id TEXT NOT NULL,
  content_json TEXT NOT NULL,
  sort_at TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  unread INTEGER NOT NULL DEFAULT 1,
  starred INTEGER NOT NULL DEFAULT 0,
  archived INTEGER NOT NULL DEFAULT 0,
  UNIQUE(feed_id, external_id)
) STRICT;
CREATE INDEX news_items_page ON news_items(user_id, sort_at DESC, id DESC);
CREATE INDEX news_items_feed ON news_items(user_id, feed_id);
"}
   {:version 19
    :sql "CREATE TABLE email_accounts (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  settings_json TEXT NOT NULL,
  encrypted_password TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_sync_at TEXT,
  next_sync_at TEXT NOT NULL,
  last_error TEXT,
  deleted_at TEXT
) STRICT;
CREATE INDEX email_accounts_due ON email_accounts(next_sync_at) WHERE deleted_at IS NULL;
CREATE TABLE email_folders (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  account_id TEXT NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  role TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  UNIQUE(account_id,name)
) STRICT;
CREATE TABLE email_messages (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  account_id TEXT NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
  content_hash TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  thread_key TEXT NOT NULL,
  sort_at TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  UNIQUE(account_id,content_hash)
) STRICT;
CREATE INDEX email_messages_page ON email_messages(user_id,sort_at DESC,id DESC);
CREATE INDEX email_messages_thread ON email_messages(user_id,account_id,thread_key);
CREATE TABLE email_locations (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  account_id TEXT NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  folder_id TEXT NOT NULL REFERENCES email_folders(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  identity TEXT NOT NULL,
  read INTEGER NOT NULL,
  flagged INTEGER NOT NULL,
  answered INTEGER NOT NULL,
  draft INTEGER NOT NULL,
  deleted INTEGER NOT NULL,
  UNIQUE(account_id,path)
) STRICT;
CREATE INDEX email_locations_message ON email_locations(user_id,message_id);
CREATE TABLE email_operations (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  account_id TEXT NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  state TEXT NOT NULL,
  request_id TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(user_id,request_id)
) STRICT;
CREATE INDEX email_operations_pending ON email_operations(user_id,account_id,state);
"}
   {:version 20
    :sql "ALTER TABLE email_operations ADD COLUMN sync_state TEXT NOT NULL DEFAULT 'pending';
CREATE INDEX email_operations_sync ON email_operations(user_id,account_id,sync_state,state);
"}
   {:version 21
    :sql "CREATE TABLE notes_categories (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  metadata_json TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT
) STRICT;
CREATE INDEX notes_categories_user ON notes_categories(user_id,deleted_at);
"}
   {:version 22
    :sql "ALTER TABLE chat_conversations ADD COLUMN fork_thread_id TEXT;
ALTER TABLE chat_conversations ADD COLUMN fork_turn_id TEXT;
ALTER TABLE chat_conversations ADD COLUMN branch_source_id TEXT;
ALTER TABLE chat_conversations ADD COLUMN branch_request_id TEXT;
CREATE UNIQUE INDEX chat_branch_request ON chat_conversations(user_id,branch_source_id,branch_request_id);
"}
   {:version 23
    :sql "CREATE TABLE chat_attachments (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  conversation_id TEXT NOT NULL REFERENCES chat_conversations(id) ON DELETE CASCADE,
  turn_id TEXT REFERENCES chat_turns(id) ON DELETE CASCADE,
  filename TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  kind TEXT NOT NULL,
  size INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  storage_path TEXT NOT NULL,
  source_attachment_id TEXT,
  state TEXT NOT NULL DEFAULT 'prepared',
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
) STRICT;
CREATE INDEX chat_attachments_scope ON chat_attachments(user_id,conversation_id,turn_id,id);
CREATE INDEX chat_attachments_storage ON chat_attachments(user_id,storage_path);
"}
   {:version 24
    :sql "CREATE TABLE todo_dav_state (
  list_id TEXT PRIMARY KEY REFERENCES todo_lists(id) ON DELETE CASCADE,
  epoch TEXT NOT NULL,
  sequence INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE todo_dav_changes (
  list_id TEXT NOT NULL REFERENCES todo_lists(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
  PRIMARY KEY(list_id,task_id)
) STRICT;
CREATE INDEX todo_dav_changes_sequence ON todo_dav_changes(list_id,sequence);
INSERT INTO todo_dav_state(list_id,epoch,sequence)
SELECT l.id,lower(hex(randomblob(16))),(SELECT count(*) FROM todo_tasks t WHERE t.list_id=l.id) FROM todo_lists l;
INSERT INTO todo_dav_changes(list_id,task_id,sequence,deleted)
SELECT list_id,id,row_number() OVER (PARTITION BY list_id ORDER BY id),CASE WHEN json_extract(metadata_json,'$.deletedAt') IS NULL THEN 0 ELSE 1 END FROM todo_tasks;
"}
   {:version 25
    :sql "UPDATE preferences SET value_json=json_set(value_json,'$.layout',json((
  SELECT json_group_array(json(CASE json_extract(entry.value,'$.id')
    WHEN 'todo.tasks' THEN json_set(entry.value,'$.id','todo.upcoming')
    WHEN 'notes.recent' THEN json_set(entry.value,'$.id','notes.list')
    WHEN 'prayer.today' THEN json_set(entry.value,'$.id','prayer.times')
    ELSE entry.value END)) FROM json_each(preferences.value_json,'$.layout') entry
)), '$.activeId',CASE json_extract(value_json,'$.activeId')
  WHEN 'todo.tasks' THEN 'todo.upcoming' WHEN 'notes.recent' THEN 'notes.list'
  WHEN 'prayer.today' THEN 'prayer.times' ELSE COALESCE(json_extract(value_json,'$.activeId'),'') END),
  revision=revision+1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
WHERE namespace='dashboard' AND key='layout' AND EXISTS (
  SELECT 1 FROM json_each(preferences.value_json,'$.layout') entry
  WHERE json_extract(entry.value,'$.id') IN ('todo.tasks','notes.recent','prayer.today')
);
"}
   {:version 26
    :sql "ALTER TABLE news_feeds ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1));
ALTER TABLE news_feeds ADD COLUMN ordering INTEGER NOT NULL DEFAULT 0 CHECK(ordering IN (0,1,2));
ALTER TABLE news_feeds ADD COLUMN keyword_filters TEXT NOT NULL DEFAULT '';
ALTER TABLE news_feeds ADD COLUMN update_mode INTEGER NOT NULL DEFAULT 0 CHECK(update_mode IN (0,1));"}
   {:version 27
    :sql "ALTER TABLE news_feeds ADD COLUMN full_text_enabled INTEGER NOT NULL DEFAULT 0 CHECK(full_text_enabled IN (0,1));"}
   {:version 28
    :sql "ALTER TABLE news_feeds ADD COLUMN encrypted_credentials TEXT;"}
   {:version 29
    :sql "ALTER TABLE news_feeds ADD COLUMN last_refresh_article_count INTEGER NOT NULL DEFAULT 0 CHECK(last_refresh_article_count>=0);"}
   {:version 30
    :sql "CREATE TABLE chat_native_deletions (
user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
thread_id TEXT NOT NULL,
parent_thread_id TEXT,
attempts INTEGER NOT NULL DEFAULT 0,
next_attempt_at TEXT NOT NULL,
last_error TEXT,
created_at TEXT NOT NULL,
PRIMARY KEY(user_id,thread_id)
) STRICT;"}
   {:version 31
    :sql "UPDATE email_accounts SET revision=revision+1 WHERE id IN (
SELECT target.account_id FROM email_folders target WHERE target.role<>'other' AND EXISTS (
SELECT 1 FROM email_folders earlier WHERE earlier.user_id=target.user_id AND earlier.account_id=target.account_id AND earlier.role=target.role AND (earlier.name<target.name OR (earlier.name=target.name AND earlier.id<target.id))));
UPDATE email_messages SET revision=revision+1 WHERE id IN (
SELECT location.message_id FROM email_locations location JOIN email_folders target ON target.id=location.folder_id WHERE target.role<>'other' AND EXISTS (
SELECT 1 FROM email_folders earlier WHERE earlier.user_id=target.user_id AND earlier.account_id=target.account_id AND earlier.role=target.role AND (earlier.name<target.name OR (earlier.name=target.name AND earlier.id<target.id))));
UPDATE email_folders AS target SET role='other',revision=revision+1 WHERE target.role<>'other' AND EXISTS (
SELECT 1 FROM email_folders earlier WHERE earlier.user_id=target.user_id AND earlier.account_id=target.account_id AND earlier.role=target.role AND (earlier.name<target.name OR (earlier.name=target.name AND earlier.id<target.id)));
CREATE UNIQUE INDEX email_folder_special_role ON email_folders(user_id,account_id,role) WHERE role<>'other';"}
   {:version 32
    :sql "ALTER TABLE email_accounts ADD COLUMN folders_initialized INTEGER NOT NULL DEFAULT 0 CHECK(folders_initialized IN (0,1));
ALTER TABLE email_accounts ADD COLUMN local_sync_state INTEGER NOT NULL DEFAULT 0 CHECK(local_sync_state IN (0,1));
UPDATE email_accounts SET folders_initialized=1 WHERE EXISTS (SELECT 1 FROM email_folders WHERE account_id=email_accounts.id AND user_id=email_accounts.user_id);"}
   {:version 33
    :sql "ALTER TABLE news_items ADD COLUMN viewed_at TEXT;
CREATE INDEX news_items_recent ON news_items(user_id,viewed_at DESC,id DESC) WHERE viewed_at IS NOT NULL;"}
   {:version 34
    :sql "ALTER TABLE chat_credentials ADD COLUMN auth_mode TEXT CHECK(auth_mode IN ('apiKey','chatgpt'));
ALTER TABLE chat_credentials ADD COLUMN native_auth_revision INTEGER;
ALTER TABLE chat_credentials ADD COLUMN native_auth_capture INTEGER NOT NULL DEFAULT 0 CHECK(native_auth_capture IN (0,1));
UPDATE chat_credentials SET auth_mode='apiKey' WHERE encrypted_value IS NOT NULL;"}
   {:version 35
    :sql "ALTER TABLE notifications ADD COLUMN dismissed_at TEXT;
DROP INDEX notifications_user_dedupe;
CREATE UNIQUE INDEX notifications_user_app_dedupe ON notifications(user_id,app_id,dedupe_key) WHERE dedupe_key IS NOT NULL;
CREATE INDEX notifications_active ON notifications(user_id,created_at DESC,id DESC) WHERE dismissed_at IS NULL;"}])

(defn- applied-versions [^js database]
  (let [statement (.prepare database "SELECT version FROM dar_migrations ORDER BY version")]
    (into #{} (map #(.-version %) (.all statement)))))

(defn- require-supported-migrations! [^js database]
  (when (.get (.prepare database "SELECT name FROM sqlite_schema WHERE type = 'table' AND name = 'dar_migrations'"))
    (let [supported (set (map :version migrations))
          unknown (remove supported (applied-versions database))]
      (when (seq unknown)
        (throw (ex-info "The database belongs to an unsupported Dar version."
                        {:type :unsupported-database :versions (vec (sort unknown))}))))))

(defn migrate! [^js database]
  (require-supported-migrations! database)
  (.exec database
         "CREATE TABLE IF NOT EXISTS dar_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL) STRICT;")
  (let [applied (applied-versions database)
        record (.prepare database
                         "INSERT INTO dar_migrations(version, applied_at) VALUES (?, ?)")]
    (doseq [{:keys [version sql]} migrations
            :when (not (contains? applied version))]
      (.exec database "BEGIN IMMEDIATE")
      (try
        (.exec database sql)
        (.run record version (.toISOString (js/Date.)))
        (.exec database "COMMIT")
        (catch :default error
          (.exec database "ROLLBACK")
          (throw error))))))

(defn open! [database-path]
  (let [database (new DatabaseSync database-path)]
    (try
      ;; Refuse future generations before changing journal mode or schema.
      (require-supported-migrations! database)
      (.exec database "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")
      (.enableDefensive database true)
      ;; SQLite's built-in lower() folds ASCII only. Keep indexed/filtering
      ;; comparisons consistent with the Unicode folding used by app code.
      ;; The connection owns this deterministic function until it closes.
      (.function database "dar_casefold" #js {:deterministic true :directOnly true}
                 (fn [value] (str/lower-case (str (or value "")))))
      (migrate! database)
      database
      (catch :default error
        (.close database)
        (throw error)))))

(defn writer-queue
  "Serializes short synchronous database mutations without hiding transactions."
  [^js database]
  (let [tail (atom (js/Promise.resolve nil))]
    #js {:write
         (fn [operation]
           (let [run (fn []
                       (try
                         (let [result (operation database)]
                           (if (and (some? result) (fn? (.-then ^js result)))
                             (js/Promise.reject
                              (ex-info "Database writer operations must be synchronous."
                                       {:type :asynchronous-database-write}))
                             (js/Promise.resolve result)))
                         (catch :default error
                           (js/Promise.reject error))))
                 next (.then @tail run run)]
             (reset! tail (.catch next (fn [_] nil)))
             next))}))

(def database-plugin
  (cordis/plugin
   {:name "dar-database"
    :inject ["instance"]
    :apply (fn [^js ctx _config]
             (let [database (open! (.-databasePath (.-instance ctx)))
                   service #js {:connection database
                                :writer (writer-queue database)}]
               (cordis/provide! ctx "database" service)
               (cordis/effect! ctx "dar.database.close"
                               (fn []
                                 (fn [] (.close database))))
               nil))}))
