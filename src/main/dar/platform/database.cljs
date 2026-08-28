(ns dar.platform.database
  (:require [dar.cordis :as cordis]
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
) STRICT;"}])

(defn- applied-versions [^js database]
  (let [statement (.prepare database "SELECT version FROM dar_migrations ORDER BY version")]
    (into #{} (map #(.-version %) (.all statement)))))

(defn migrate! [^js database]
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
      (.exec database "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")
      (.enableDefensive database true)
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
