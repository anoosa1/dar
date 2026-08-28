(ns dar.platform.auth
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            ["node:buffer" :refer [Buffer]]
            ["node:crypto" :as crypto]))

(def session-cookie "dar-session")
(def session-lifetime-ms (* 30 24 60 60 1000))

(defn random-token []
  (.toString (crypto/randomBytes 32) "base64url"))

(defn token-hash [token]
  (.digest (doto (crypto/createHash "sha256") (.update token "utf8"))))

(defn parse-cookies [header]
  (if (str/blank? header)
    {}
    (into {}
          (keep (fn [part]
                  (let [[name value] (str/split (str/trim part) #"=" 2)]
                    (when (and (not (str/blank? name)) (some? value))
                      [name value]))))
          (str/split header #";"))))

(defn csrf-valid? [^js session supplied]
  (when (and session (not (str/blank? supplied)))
    (let [expected (.-csrfHash session)
          actual (token-hash supplied)]
      (and (= (.-length expected) (.-length actual))
           (crypto/timingSafeEqual expected actual)))))

(defn- session-service [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        insert-session (.prepare
                        connection
                        "INSERT INTO sessions(id_hash, user_id, csrf_hash, expires_at, created_at, last_seen_at) VALUES (?, ?, ?, ?, ?, ?)")
        find-session (.prepare
                      connection
                      "SELECT s.user_id, s.csrf_hash, s.expires_at, u.subject, u.display_name, u.email, u.administrator FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id_hash = ? AND s.expires_at > ?")
        delete-session (.prepare connection "DELETE FROM sessions WHERE id_hash = ?")
        delete-expired (.prepare connection "DELETE FROM sessions WHERE expires_at <= ?")
        upsert-user (.prepare
                     connection
                     "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(subject) DO UPDATE SET display_name = excluded.display_name, email = excluded.email, administrator = excluded.administrator, updated_at = excluded.updated_at")
        find-user (.prepare
                   connection
                   "SELECT id, subject, display_name, email, administrator FROM users WHERE subject = ?")]
    #js {:createSession
         (fn [user-id]
           (let [token (random-token)
                 csrf-token (random-token)
                 now (js/Date.)
                 expires (js/Date. (+ (.getTime now) session-lifetime-ms))]
             (.write writer
                     (fn [_]
                       (.run insert-session
                             (token-hash token) user-id (token-hash csrf-token)
                             (.toISOString expires) (.toISOString now) (.toISOString now))
                       #js {:token token
                            :csrfToken csrf-token
                            :expiresAt (.toISOString expires)}))))
         :findSession
         (fn [token]
           (when-not (str/blank? token)
             (when-let [row (.get find-session (token-hash token) (.toISOString (js/Date.)))]
               #js {:userId (.-user_id row)
                    :csrfHash (.-csrf_hash row)
                    :expiresAt (.-expires_at row)
                    :user #js {:id (.-user_id row)
                               :subject (.-subject row)
                               :displayName (.-display_name row)
                               :email (.-email row)
                               :administrator (= 1 (.-administrator row))}})))
         :deleteSession
         (fn [token]
           (.write writer (fn [_] (.run delete-session (token-hash token)) nil)))
         :deleteExpired
         (fn []
           (.write writer
                   (fn [_]
                     (.-changes (.run delete-expired (.toISOString (js/Date.)))))))
         :upsertUser
         (fn [^js claims]
           (let [now (.toISOString (js/Date.))
                 id (crypto/randomUUID)]
             (.write writer
                     (fn [_]
                       (.run upsert-user id (.-subject claims) (.-displayName claims)
                             (.-email claims) (if (.-administrator claims) 1 0) now now)
                       (let [row (.get find-user (.-subject claims))]
                         #js {:id (.-id row)
                              :subject (.-subject row)
                              :displayName (.-display_name row)
                              :email (.-email row)
                              :administrator (= 1 (.-administrator row))})))))}))

(defn request-cookie [^js request name]
  (get (parse-cookies (.get (.-headers request) "cookie")) name))

(defn- attach-session! [^js auth ^js request]
  (when-let [^js session ((.-findSession auth) (request-cookie request session-cookie))]
    (set! (.-session request) session)
    (set! (.-user request) (.-user session)))
  request)

(defn- csrf-protected? [^js request]
  (and (str/starts-with? (.-path request) "/api/v1")
       (contains? #{"POST" "PUT" "PATCH" "DELETE"}
                  (str/upper-case (.-method request)))))

(defn- register-auth! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js http-security (.-httpSecurity ctx)
        auth (session-service (.-database ctx))]
    (cordis/provide! ctx "auth" auth)
    (.use server
          (fn [^js request response next]
            ((.-handle http-security)
             request response
             (fn []
               (attach-session! auth request)
               (if (csrf-protected? request)
                 (cond
                   (nil? (.-session request))
                   (http/error! response 401 "unauthenticated"
                                "Authentication is required." {} (.-requestId request))

                   (not (csrf-valid? (.-session request)
                                     (.get (.-headers request) "x-csrf-token")))
                   (http/error! response 403 "forbidden"
                                "The CSRF token is missing or invalid." {} (.-requestId request))

                   :else (next))
                 (next))))))
    (.get server "/api/v1/session"
          (fn [^js request response _next]
            (if-let [^js session (.-session request)]
              (http/json! response
                          {:user (js->clj (.-user session) :keywordize-keys true)
                           :expiresAt (.-expiresAt session)})
              (http/error! response 401 "unauthenticated"
                           "Authentication is required." {} (.-requestId request)))))
    nil))

(def auth-plugin
  (cordis/plugin
   {:name "dar-auth"
    :inject ["database" "server" "httpSecurity"]
    :apply register-auth!}))
