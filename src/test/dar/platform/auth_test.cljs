(ns dar.platform.auth-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.auth :as auth]
            [dar.platform.database :as database]
            [dar.cordis :as cordis]
            ["node:buffer" :refer [Buffer]]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest parses-cookie-headers-without-decoding-token-material
  (is (= {"dar-session" "abc.def_123" "theme" "dark"}
         (auth/parse-cookies "theme=dark; dar-session=abc.def_123")))
  (is (= {} (auth/parse-cookies nil))))

(deftest validates-csrf-tokens-in-constant-time
  (let [token "csrf-token"
        session #js {:csrfHash (auth/token-hash token)}]
    (is (true? (auth/csrf-valid? session token)))
    (is (false? (boolean (auth/csrf-valid? session "wrong"))))
    (is (false? (boolean (auth/csrf-valid? nil token))))))

(deftest generates-url-safe-high-entropy-tokens
  (let [first-token (auth/random-token)
        second-token (auth/random-token)]
    (is (re-matches #"[A-Za-z0-9_-]{43}" first-token))
    (is (not= first-token second-token))
    (is (= 32 (.-length (.from Buffer first-token "base64url"))))))

(deftest session-middleware-enforces-expiry-csrf-and-live-identity-claims
  (async done
    (let [root (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-auth-boundary-"))
          db (database/open! (.resolve path root "dar.sqlite")) ctx (cordis/context)
          routes (atom {}) middleware (atom nil) account (atom nil) session (atom nil)
          invoke (fn [method cookie csrf & [target]]
                   (let [body (atom nil) routed (atom false)
                         request #js {:method method :path (or target "/api/v1/session") :requestId "fixture"
                                      :headers (js/Map. (clj->js (cond-> [] cookie (conj ["cookie" cookie]) csrf (conj ["x-csrf-token" csrf]))))}
                         response #js {:headers (js/Map.) :json #(reset! body (js->clj % :keywordize-keys true))}]
                     (@middleware request response
                                  #(do (reset! routed true)
                                       (when-let [handler (get @routes (.-path request))] (handler request response nil))))
                     {:routed @routed :status (.-status response) :body @body}))]
      (cordis/provide! ctx "database" #js {:connection db :writer (database/writer-queue db)})
      (cordis/provide! ctx "server" #js {:use #(reset! middleware %) :get #(swap! routes assoc %1 %2)})
      (cordis/provide! ctx "httpSecurity" #js {:handle (fn [_ _ next] (next))})
      (-> (cordis/mount! ctx auth/auth-plugin {})
          (.then (fn [_]
                   (.upsertUser (.-auth ctx) #js {:subject "owner" :displayName "Owner" :email "owner@example.test" :administrator false})))
          (.then (fn [user] (reset! account user) (.createSession (.-auth ctx) (.-id user))))
          (.then (fn [created]
                   (reset! session created)
                   (let [row (.get (.prepare db "SELECT id_hash,csrf_hash FROM sessions"))
                         cookie (str "dar-session=" (.-token created))]
                     (is (= 32 (.-length (.-id_hash row))))
                     (is (not= (.-token created) (.toString (.-id_hash row) "utf8")))
                     (is (not= (.-csrfToken created) (.toString (.-csrf_hash row) "utf8")))
                     (is (= (.-id @account) (get-in (invoke "GET" cookie nil) [:body :user :id])))
                     (is (= "unauthenticated" (get-in (invoke "POST" nil nil) [:body :error :code])))
                     (is (= "forbidden" (get-in (invoke "POST" cookie nil) [:body :error :code])))
                     (is (= "forbidden" (get-in (invoke "PUT" cookie "wrong") [:body :error :code])))
                     (is (true? (:routed (invoke "PATCH" cookie (.-csrfToken created)))))
                     (is (= 401 (:status (invoke "GET" "dar-session=unknown" nil)))))
                   (doseq [path ["/api/v1/settings/personal" "/api/v1/location/search" "/api/v1/dashboard/catalog"]]
                     (is (= 401 (:status (invoke "GET" nil nil path))))
                     (is (true? (:routed (invoke "GET" (str "dar-session=" (.-token created)) nil path)))))
                   (doseq [path ["/api/v1/meta" "/api/v1/auth/login" "/api/v1/auth/callback" "/share/token" "/dav/files/owner"]]
                     (is (true? (:routed (invoke "GET" nil nil path)))))
                   (is (= 401 (:status (invoke "POST" nil nil "/api/v1/meta"))))
                   (.upsertUser (.-auth ctx) #js {:subject "owner" :displayName "Updated" :email "changed@example.test" :administrator true})))
          (.then (fn [updated]
                   (is (= (.-id @account) (.-id updated)))
                   (let [user (get-in (invoke "GET" (str "dar-session=" (.-token @session)) nil) [:body :user])]
                     (is (= "Updated" (:displayName user)))
                     (is (= "changed@example.test" (:email user)))
                     (is (true? (:administrator user))))
                   (.upsertUser (.-auth ctx) #js {:subject "reader" :displayName "Reader" :email "reader@example.test" :administrator false})))
          (.then (fn [reader] (.createSession (.-auth ctx) (.-id reader))))
          (.then (fn [reader-session]
                   (.run (.prepare db "UPDATE sessions SET expires_at='2000-01-01T00:00:00.000Z' WHERE id_hash=?") (auth/token-hash (.-token @session)))
                   (is (= 401 (:status (invoke "GET" (str "dar-session=" (.-token @session)) nil))))
                   (is (false? (get-in (invoke "GET" (str "dar-session=" (.-token reader-session)) nil) [:body :user :administrator])))
                   (-> (.deleteExpired (.-auth ctx))
                       (.then (fn [deleted]
                                (is (= 1 deleted))
                                (is (= 1 (.-count (.get (.prepare db "SELECT count(*) AS count FROM sessions")))))
                                (.deleteSession (.-auth ctx) (.-token reader-session))))
                       (.then (fn [_]
                                (is (= 401 (:status (invoke "GET" (str "dar-session=" (.-token reader-session)) nil)))))))))
          (.catch #(is false (str %)))
          (.finally (fn []
                      (-> (cordis/dispose! ctx)
                          (.finally (fn [] (.close db) (.rmSync fs root #js {:recursive true :force true}) (done))))))))))
