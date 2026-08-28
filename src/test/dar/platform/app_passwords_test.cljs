(ns dar.platform.app-passwords-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.app-passwords :as app-passwords]
            [dar.platform.database :as database]
            ["node:buffer" :refer [Buffer]]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(defn- open-store! []
  (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-app-passwords-test-"))
        connection (database/open! (.resolve path directory "dar.sqlite"))]
    {:directory directory
     :connection connection
     :store (app-passwords/service #js {:connection connection
                                        :writer (database/writer-queue connection)})}))

(defn- close-store! [fixture]
  (.close (:connection fixture))
  (.rmSync fs (:directory fixture) #js {:recursive true :force true}))

(defn- seed-user! [^js connection subject]
  (.run (.prepare connection
                  "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?)")
        "user-1" subject "Alice" "alice@example.test"
        (.toISOString (js/Date.)) (.toISOString (js/Date.))))

(defn- sync-error-type [thunk]
  (try
    (thunk)
    :no-error
    (catch :default error
      (:type (ex-data error)))))

(defn- settle
  "Maps a promise to a resolved {:outcome ...} step that never rejects."
  [promise outcome]
  (-> promise
      (.then (fn [value] #js {:outcome outcome :value value}))
      (.catch (fn [error] #js {:outcome outcome :error error}))))

(defn- run-steps
  "Reduces promise-returning thunks left to right; a rejecting thunk fails the
  whole run. Each thunk owns its own assertions."
  [thunks]
  (reduce (fn [prior thunk] (.then prior thunk))
          (js/Promise.resolve nil)
          thunks))

(defn- finish
  "Ends an async test exactly once, closing the store first on either path."
  [start fixture done]
  (-> start
      (.then (fn [_]
               (close-store! fixture)
               (done)))
      (.catch (fn [error]
                (is false (str "flow failed: " (.-message error)))
                (close-store! fixture)
                (done)))))

(deftest rejects-invalid-labels-and-scopes
  (let [fixture (open-store!)]
    (try
      (let [store (:store fixture)]
        (is (thrown-with-msg? js/Error #"label or scopes"
                              (.create store "user-1" "" ["files"])))
        (is (thrown-with-msg? js/Error #"label or scopes"
                              (.create store "user-1" "   " ["files"])))
        (is (thrown-with-msg? js/Error #"label or scopes"
                              (.create store "user-1" "Laptop" [])))
        (is (thrown-with-msg? js/Error #"label or scopes"
                              (.create store "user-1" "Laptop" ["files" "files"])))
        (is (thrown-with-msg? js/Error #"label or scopes"
                              (.create store "user-1" "Laptop" ["files" "email"])))
        (is (= :invalid-app-password
               (sync-error-type #(.create store "user-1" "Laptop" ["files" "calendar" "extra"])))))
      (finally
        (close-store! fixture)))))

(defn- assert-created-record [token created]
  (reset! token (:token created))
  (is (re-matches #"[A-Za-z0-9_-]{43}" (:token created)))
  (is (= "Laptop" (:label created)))
  (is (= ["files" "calendar"] (:scopes created)))
  (is (= 1 (:revision created)))
  (is (nil? (:lastUsedAt created)))
  (is (re-matches #"[A-Za-z0-9_-]{12}…" (:prefix created))))

(defn- create-step [store token]
  (fn []
    (let [created-promise (.create store "user-1" "Laptop" ["files" "calendar"])]
      (.then created-promise (fn [created] (assert-created-record token created))))))

(defn- verify-step [store subject token]
  (fn []
    (let [verify-promise (.verify store subject @token)]
      (.then verify-promise
             (fn [match]
               (is (= subject (aget (:user match) "subject")))
               (is (= ["files" "calendar"]
                      (js->clj (aget (:appPassword match) "scopes")))))))))

(defn- list-hides-secret-step [store]
  (fn []
    (let [listed (first (.list store "user-1"))]
      (is (nil? (:token listed)))
      (is (some? (:lastUsedAt listed)))
      (js/Promise.resolve nil))))

(defn- verify-mismatch-steps [store]
  [(fn []
     (let [wrong-promise (.verify store "alice-subject" "wrong-token-value-000000000000")]
       (.then wrong-promise (fn [wrong-token] (is (nil? wrong-token))))))
   (fn []
     (let [unknown-promise (.verify store "unknown-subject" "anything")]
       (.then unknown-promise (fn [unknown] (is (nil? unknown))))))])

(deftest creates-lists-and-verifies-without-disclosing-secrets
  (async done
    (let [fixture (open-store!)]
      (seed-user! (:connection fixture) "alice-subject")
      (let [store (:store fixture)
            token (atom nil)
            steps (into [(create-step store token)
                         (verify-step store "alice-subject" token)
                         (list-hides-secret-step store)]
                        (verify-mismatch-steps store))]
        (finish (run-steps steps) fixture done)))))

(defn- assert-revoke-outcomes [results]
  (let [[missing stale current] (js->clj results :keywordize-keys true)]
    (is (= :missing (:outcome missing)))
    (is (= :app-password-not-found (:type (ex-data (:error missing)))))
    (is (= :stale (:outcome stale)))
    (is (= :app-password-conflict (:type (ex-data (:error stale)))))
    (is (= :current (:outcome current)))
    (is (true? (:value current)))))

(defn- revoke-steps [store token id]
  [#(let [created-promise (.create store "user-1" "Phone" ["contacts"])]
      (.then created-promise
             (fn [created]
               (reset! token (:token created))
               (reset! id (:id created)))))
   ;; A zero revision fails validation before any write. The writer queue runs
   ;; the remaining revokes in order, so the stale revision is still current
   ;; when it is evaluated.
   (fn []
     (is (= :invalid-precondition
            (sync-error-type #(.revoke store "user-1" @id 0))))
     (let [revokes-promise
           (js/Promise.all
            #js [(settle (.revoke store "user-1" "missing-id" 1) :missing)
                 (settle (.revoke store "user-1" @id 2) :stale)
                 (settle (.revoke store "user-1" @id 1) :current)])]
       (.then revokes-promise assert-revoke-outcomes)))
   (fn []
     (let [verify-promise (.verify store "bob-subject" @token)]
       (.then verify-promise
              (fn [after-revoke]
                (is (nil? after-revoke))
                (is (empty? (.list store "user-1")))))))])

(deftest revokes-only-with-a-current-if-match-revision
  (async done
    (let [fixture (open-store!)]
      (seed-user! (:connection fixture) "bob-subject")
      (let [store (:store fixture)]
        (finish (run-steps (revoke-steps store (atom nil) (atom nil))) fixture done)))))

(deftest parsing-basic-credentials-is-conservative
  (let [request (fn [header]
                  #js {:headers #js {:get (fn [name]
                                            (when (= name "authorization") header))}})]
    (is (= {:username "alice" :token "secret-token"}
           (app-passwords/basic-credentials
            (request (str "Basic " (.toString (js/Buffer.from "alice:secret-token") "base64"))))))
    (is (nil? (app-passwords/basic-credentials (request "Bearer abc"))))
    (is (nil? (app-passwords/basic-credentials (request "Basic !!!!not-base64!!!!"))))
    (is (nil? (app-passwords/basic-credentials (request nil))))
    (is (nil? (app-passwords/basic-credentials
               (request (str "Basic " (.toString (js/Buffer.from "no-token-part") "base64"))))))))

(deftest scope-check-is-exact
  (is (true? (app-passwords/has-scope? #js {:scopes ["files"]} "files")))
  (is (false? (app-passwords/has-scope? #js {:scopes ["files"]} "calendar")))
  (is (false? (app-passwords/has-scope? #js {:scopes ["calendar"]} "files.dav"))))
