(ns dar.platform.app-passwords
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.platform.auth :as auth]
            [dar.server.http :as http]
            [malli.core :as m]
            ["node:buffer" :refer [Buffer]]
            ["node:crypto" :refer [randomUUID]]))

;; Protocol clients authenticate with scoped Basic app passwords. The secret is
;; generated once, shown to the operator immediately, and only ever stored as a
;; SHA-256 hash plus a short display prefix (same treatment as sessions).

(def allowed-scopes
  #{"files" "calendar" "contacts" "todo"})

(def create-schema
  [:map {:closed true}
   [:label [:and
            [:string {:min 1 :max 64}]
            [:fn (fn [label] (not (str/blank? (str/trim label))))]]]
   [:scopes [:and
             [:vector [:enum "files" "calendar" "contacts" "todo"]]
             [:fn (fn [scopes] (and (pos? (count scopes)) (apply distinct? scopes)))]]]])

(defn- valid-create? [value]
  (m/validate create-schema value))

(defn- prefix-of [token]
  (str (subs token 0 12) "…"))

(defn- row->record [^js row]
  {:id (.-id row)
   :label (.-label row)
   :prefix (.-prefix row)
   :scopes (js->clj (js/JSON.parse (.-scopes_json row)))
   :createdAt (.-created_at row)
   :lastUsedAt (.-last_used_at row)
   :revision (.-revision row)})

(defn- create-password!
  "Generates one secret, persists only its hash, and returns it exactly once."
  [writer insert select-one user-id label scopes]
  (when-not (and (string? user-id) (valid-create? {:label label :scopes scopes}))
    (throw (ex-info "The app password label or scopes are invalid."
                    {:type :invalid-app-password})))
  (let [clean-label (str/trim label)
        token (auth/random-token)
        now (.toISOString (js/Date.))]
    (.write writer
            (fn [_]
              (let [id (randomUUID)]
                (.run insert id user-id clean-label (auth/token-hash token)
                      (prefix-of token) (js/JSON.stringify (clj->js (vec scopes))) now)
                (assoc (row->record (.get select-one user-id id))
                       :token token))))))

(defn- list-passwords
  "Lists active app passwords; revoked rows stay hidden."
  [select-page user-id]
  (when-not (string? user-id)
    (throw (ex-info "App-password user scope is invalid."
                    {:type :invalid-app-password})))
  (mapv row->record (.all select-page user-id)))

(defn- revoke-password!
  "Revokes one password when the caller proves it saw the current revision."
  [^js writer ^js revoke-if-match ^js select-one user-id id expected-revision]
  (when-not (and (string? user-id) (string? id)
                 (int? expected-revision) (pos? expected-revision))
    (throw (ex-info "A valid If-Match revision is required."
                    {:type :invalid-precondition})))
  (.write writer
          (fn [_]
            (or (pos? (.-changes (.run revoke-if-match
                                       (.toISOString (js/Date.)) user-id id
                                       expected-revision)))
                (let [^js existing (.get select-one user-id id)]
                  (cond
                    (nil? existing)
                    (throw (ex-info "The app password does not exist."
                                    {:type :app-password-not-found}))

                    (some? (.-revoked_at existing))
                    (throw (ex-info "The app password is already revoked."
                                    {:type :app-password-conflict
                                     :revision (.-revision existing)}))

                    :else
                    (throw (ex-info "The app password changed."
                                    {:type :app-password-conflict
                                     :revision (.-revision existing)}))))))))

(defn- verify-password!
  "Matches Basic credentials against one non-revoked password and touches it."
  [writer find-user match-token touch subject token]
  (if-not (and (string? subject) (string? token) (not (str/blank? token)))
    (js/Promise.resolve nil)
    (.write writer
            (fn [_]
              (when-let [^js user (.get find-user subject)]
                (when-let [^js row (.get match-token (auth/token-hash token))]
                  (when (= (.-user_id row) (.-id user))
                    (.run touch (.toISOString (js/Date.)) (.-id row))
                    {:user #js {:id (.-id user)
                                :subject (.-subject user)
                                :displayName (.-display_name user)
                                :email (.-email user)
                                :administrator (= 1 (.-administrator user))}
                     :appPassword #js {:id (.-id row)
                                       :label (.-label row)
                                       :scopes (js->clj (js/JSON.parse (.-scopes_json row)))}})))))))

(defn service
  "Creates a user-scoped app-password store with revocation and Basic matching."
  [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        insert
        (.prepare connection
                  "INSERT INTO app_passwords(id, user_id, label, secret_hash, prefix, scopes_json, revision, created_at, last_used_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, NULL)")
        select-one
        (.prepare connection
                  "SELECT id, user_id, label, scopes_json, revision, prefix, created_at, last_used_at, revoked_at FROM app_passwords WHERE user_id = ? AND id = ?")
        select-page
        (.prepare connection
                  "SELECT id, user_id, label, scopes_json, revision, prefix, created_at, last_used_at, revoked_at FROM app_passwords WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at DESC, id DESC LIMIT 100")
        revoke-if-match
        (.prepare connection
                  "UPDATE app_passwords SET revoked_at = ?, revision = revision + 1 WHERE user_id = ? AND id = ? AND revision = ? AND revoked_at IS NULL")
        touch
        (.prepare connection
                  "UPDATE app_passwords SET last_used_at = ? WHERE id = ?")
        match-token
        (.prepare connection
                  "SELECT p.id, p.label, p.scopes_json, u.id AS user_id, u.subject, u.display_name, u.email, u.administrator FROM app_passwords p JOIN users u ON u.id = p.user_id WHERE p.secret_hash = ? AND p.revoked_at IS NULL")
        find-user
        (.prepare connection
                  "SELECT id, subject, display_name, email, administrator FROM users WHERE subject = ?")]
    #js {:create (partial create-password! writer insert select-one)
         :list (partial list-passwords select-page)
         :revoke (partial revoke-password! writer revoke-if-match select-one)
         ;; DAV and other protocol clients resolve Basic credentials against the
         ;; subject named in the URL; display names are never identities.
         :verify (partial verify-password! writer find-user match-token touch)}))

(defn has-scope? [^js app-password scope]
  (contains? (set (js->clj (.-scopes app-password))) scope))

(defn basic-credentials
  "Parses a Basic authorization header into {:username :token} or nil."
  [^js request]
  (let [header (.get (.-headers request) "authorization")]
    (when-let [[_ encoded] (and (string? header)
                                (re-matches #"(?i)^Basic\s+(.+)$" (str/trim header)))]
      (try
        (let [decoded (.toString (.from Buffer encoded "base64") "utf8")
              [username token] (str/split decoded #":" 2)]
          (when (and (not (str/blank? username)) (some? token))
            {:username username :token token}))
        (catch :default _ nil)))))

(defn- require-session! [^js request response]
  (if-let [user (.-user request)]
    user
    (do (http/error! response 401 "unauthenticated"
                     "Authentication is required." {} (.-requestId response))
        nil)))

(defn- send-error! [response error]
  (let [data (ex-data error)
        request-id (.-requestId response)]
    (case (:type data)
      :app-password-conflict
      (http/error! response 409 "conflict" (ex-message error) data request-id)
      :app-password-not-found
      (http/error! response 404 "notFound" (ex-message error) data request-id)
      (http/error! response 400 "invalid" (ex-message error) data request-id))))

(defn- if-match-revision [^js request]
  (let [header (.get (.-headers request) "if-match")]
    (when (str/blank? header)
      (throw (ex-info "An If-Match header is required."
                      {:type :invalid-precondition})))
    (let [parsed (js/Number (str/replace (str/trim header) #"\"" ""))]
      (when-not (and (int? parsed) (pos? parsed))
        (throw (ex-info "The If-Match header is invalid."
                        {:type :invalid-precondition})))
      parsed)))

(defn- register-api! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js store (.-appPasswords ctx)]
    (.get server "/api/v1/app-passwords"
          (fn [^js request response _next]
            (when (require-session! request response)
              (http/json! response {:items (.list store (.-id (.-user request)))}))))
    (.post server "/api/v1/app-passwords"
           (fn [^js request response _next]
             (when (require-session! request response)
               (-> (.json request)
                   (.then (fn [body]
                            (let [value (js->clj body :keywordize-keys true)]
                              (.create store
                                       (.-id (.-user request))
                                       (:label value)
                                       (vec (:scopes value))))))
                   (.then (fn [record]
                            (http/json! response 201 record)))
                   (.catch (fn [error]
                             (send-error! response error)))))))
    (.delete server "/api/v1/app-passwords/:id"
             (fn [^js request response _next]
               (when (require-session! request response)
                 (-> (js/Promise.resolve)
                     (.then (fn []
                              (.revoke store
                                       (.-id (.-user request))
                                       (.-id (.-params request))
                                       (if-match-revision request))))
                     (.then (fn [_]
                              (http/json! response {:ok true})))
                     (.catch (fn [error]
                               (send-error! response error)))))))
    nil))

(def app-passwords-plugin
  (cordis/plugin
   {:name "dar-app-passwords"
    :inject ["database" "server"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "appPasswords" (service (.-database ctx)))
      (register-api! ctx _config))}))
