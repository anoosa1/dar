(ns dar.platform.activity-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.activity :as activity]
            [dar.platform.database :as database]
            [dar.cordis :as cordis]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest records-recipients-scoped-feed-and-outbox
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-activity-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (activity/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})
          cleanup! (fn []
                     (.close connection)
                     (.rmSync fs directory #js {:recursive true :force true})
                     (done))]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES
              ('user-1', 'subject-1', 'User One', 'one@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'),
              ('user-2', 'subject-2', 'User Two', 'two@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (.then
       (.record service "user-1" "files" "fileCreated"
                {:path "reports/quarter.pdf"} ["user-1" "user-2" "user-1"])
       (fn [event]
         (is (string? (:id event)))
         (is (= 1 (count (.list service "user-1"))))
         (is (= 1 (count (.list service "user-2"))))
         (is (true? (.canSee service "user-1" (:id event))))
         (is (false? (.canSee service "user-3" (:id event))))
         (is (= 1 (count (.dueOutbox service (.toISOString (js/Date.))))))
         (.then
          (.markDelivered service (:id event) (.toISOString (js/Date.)))
          (fn [delivered]
            (is (true? delivered))
            (is (= 0 (count (.dueOutbox service (.toISOString (js/Date.))))))
            (cleanup!))))
       (fn [error]
         (js/console.error error)
         (is false (str error))
         (cleanup!)))))
  )

(deftest filters-and-pages-activity-with-an-opaque-cursor
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-activity-page-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (activity/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})
          cleanup! (fn []
                     (.close connection)
                     (.rmSync fs directory #js {:recursive true :force true})
                     (done))]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at)
              VALUES ('user-1', 'subject-1', 'User One', 'one@example.test', 0,
                      '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.record service "user-1" "files" "fileCreated"
                   {:path "/one.txt"} ["user-1"])
          (.then (fn [_]
                   (.record service "user-1" "calendar" "eventCreated"
                            {:path "/one.ics"} ["user-1"])))
          (.then (fn [_]
                   (.record service "user-1" "files" "fileEdited"
                            {:path "/two.txt"} ["user-1"])))
          (.then (fn [_]
                   (let [first-page (.page service "user-1"
                                           {:app-id "files" :limit 1})]
                     (is (= 1 (count (:items first-page))))
                     (is (string? (:nextCursor first-page)))
                     (let [second-page (.page service "user-1"
                                              {:app-id "files"
                                               :limit 1
                                               :cursor (:nextCursor first-page)})]
                       (is (= 1 (count (:items second-page))))
                       (is (nil? (:nextCursor second-page)))
                       (is (not= (-> first-page :items first :id)
                                 (-> second-page :items first :id))))
                     (let [path-page (.page service "user-1"
                                            {:app-id "files" :path "/one.txt"})]
                       (is (= ["fileCreated"]
                              (mapv :eventType (:items path-page))))))))
          (.catch (fn [error]
                    (js/console.error error)
                    (is false (str error))))
          (.finally cleanup!)))))

(deftest delivery-retries-persist-across-restart-and-follow-current-recipients
  (async done
    (let [root (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-activity-retry-"))
          file (.resolve path root "dar.sqlite")
          connection (atom (database/open! file))
          ctx (atom (cordis/context))
          clock (atom (.now js/Date))
          failed (atom true) hints (atom []) event-id (atom nil)
          service! #(activity/service #js {:connection @connection :writer (database/writer-queue @connection)})
          ^js store (service!)
          streams #js {:publish (fn [user resource]
                                  (swap! hints conj [user resource])
                                  (when (and @failed (= user "recipient")) (throw (js/Error. "fixture provider secret"))))}
          flush! (activity/delivery! @ctx store streams #(deref clock))]
      (.exec @connection "INSERT INTO users(id,subject,display_name,email,administrator,created_at,updated_at) VALUES ('owner','owner','Owner','owner@example.test',0,'2026-10-04','2026-10-04'),('recipient','recipient','Recipient','recipient@example.test',0,'2026-10-04','2026-10-04')")
      (-> (flush!)
          (.then #(.record store "owner" "todo" "listShared" {:title "Shared list"} ["owner" "recipient"]))
          (.then (fn [event]
                   (reset! event-id (:id event))
                   (reset! clock (.now js/Date))
                   (let [pass (flush!)] (is (identical? pass (flush!)) "Parallel recovery calls join one pass") pass)))
          (.then (fn [_]
                   (let [^js row (.get (.prepare @connection "SELECT * FROM activity_outbox"))]
                     (is (= 1 (.-attempts row)))
                     (is (= (+ @clock 1000) (.parse js/Date (.-available_at row))))
                     (is (= "Activity delivery failed; retry scheduled." (.-last_error row))))
                   (is (= 1 (count (.list store "owner"))))
                   (is (= 1 (count (.list store "recipient"))))
                   (reset! hints []) (flush!)))
          (.then (fn [_]
                   (is (empty? @hints) "No retry before its durable deadline")
                   (cordis/dispose! @ctx)))
          (.then (fn [_]
                   (.close @connection)
                   (reset! connection (database/open! file))
                   (reset! ctx (cordis/context))
                   ;; A revoked recipient must not receive a stale queued hint.
                   (.run (.prepare @connection "DELETE FROM activity_recipients WHERE event_id=? AND user_id='recipient'") @event-id)
                   (reset! failed false) (swap! clock + 1000)
                   (let [^js restarted (service!) pass (activity/delivery! @ctx restarted streams #(deref clock))]
                     (-> (pass)
                         (.then (fn [_]
                                  (is (= [["owner" "activity"] ["owner" "todo"] ["owner" "notifications"]] @hints))
                                  (is (empty? (.dueOutbox restarted (.toISOString (js/Date. @clock)))))
                                  (is (= 1 (count (.list restarted "owner"))))
                                  (is (empty? (.list restarted "recipient")))
                                  (.markDelivered restarted @event-id (.toISOString (js/Date. @clock)))))
                         (.then (fn [marked] (is (false? marked))))))))
          (.catch (fn [error] (is false (str error))))
          (.finally (fn []
                      (-> (cordis/dispose! @ctx)
                          (.then (fn [_] (.close @connection) (.rmSync fs root #js {:recursive true :force true}) (done))))))))))

(deftest delivery-disposal-drains-the-current-write-and-keeps-unstarted-events
  (async done
    (let [ctx (cordis/context) release (atom nil) started (atom nil) delivered (atom [])
          waiting (js/Promise. (fn [resolve] (reset! release resolve)))
          began (js/Promise. (fn [resolve] (reset! started resolve)))
          store #js {:dueOutbox (fn [_] [{:id "first" :appId "todo"} {:id "second" :appId "notes"}])
                     :recipients (fn [_] ["owner"])
                     :markDelivered (fn [id _] (swap! delivered conj id) (@started) waiting)}
          flush! (activity/delivery! ctx store #js {:publish (fn [_ _] nil)})]
      (-> began
          (.then (fn [_]
                   (let [disposed (cordis/dispose! ctx) drained (atom false)]
                     (.then disposed #(reset! drained true))
                     (is (false? @drained) "Disposal waits for the owned write")
                     (@release true)
                     disposed)))
          (.then (fn [_]
                   (is (= ["first"] @delivered) "Unstarted work stays in the durable outbox")
                   (is (nil? (flush!)) "Disposed delivery cannot restart")))
          (.catch (fn [error] (is false (str error))))
          (.finally done)))))
