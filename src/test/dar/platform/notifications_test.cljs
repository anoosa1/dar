(ns dar.platform.notifications-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.database :as database]
            [dar.platform.notifications :as notifications]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest rejects-unscoped-mutations-and-nonboolean-desktop-consent
  (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-notification-boundary-"))
        connection (database/open! (.resolve path directory "dar.sqlite"))
        service (notifications/service #js {:connection connection :writer (database/writer-queue connection)})]
    (try
      (doseq [operation [#(.create service nil "email" "received" {:title "Private"} nil)
                         #(.create service "owner" "" "received" {:title "Private"} nil)
                         #(.create service "owner" "email" "received" nil nil)
                         #(.list service nil false)
                         #(.markRead service "owner" nil)
                         #(.markAllRead service nil)
                         #(.delete service nil "private-id")
                         #(.unreadCount service nil)]]
        (try (operation) (is false "An unscoped notification operation must fail before storage")
             (catch :default error (is (= :invalid-notification (:type (ex-data error)))))))
      (doseq [enabled ["true" 1 {} nil]]
        (try (.saveSettings service "owner" {:desktopEnabled enabled} 0)
             (is false "Only explicit Boolean consent can enable desktop alerts")
             (catch :default error (is (= :invalid-notification (:type (ex-data error)))))))
      (is (zero? (.-count (.get (.prepare connection "SELECT count(*) AS count FROM notifications")))))
      (is (zero? (.-count (.get (.prepare connection "SELECT count(*) AS count FROM preferences")))))
      (finally (.close connection) (.rmSync fs directory #js {:recursive true :force true})))))

(deftest creates-deduplicates-reads-and-deletes-notifications
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-notifications-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (notifications/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES
              ('user-1', 'subject-1', 'User One', 'one@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.create service "user-1" "calendar" "eventStarting"
                   {:title "Design review"} "calendar/design-review")
          (.then (fn [created]
                   (is (string? (:id created)))
                   (.create service "user-1" "calendar" "eventStarting"
                            {:title "Design review"} "calendar/design-review")))
          (.then (fn [deduplicated]
                   (is (string? (:id deduplicated)))
                   (is (= 1 (.unreadCount service "user-1")))
                   (.markRead service "user-1" (:id deduplicated))))
          (.then (fn [_]
                   (is (= 0 (.unreadCount service "user-1")))
                   (.markAllRead service "user-1")))
          (.then (fn [changed]
                   (is (= 0 changed))
                   (.delete service "user-1" (:id (first (.list service "user-1" false))))))
          (.then (fn [_]
                   (is (= [] (.list service "user-1" false)))))
          (.catch (fn [error]
                    (is false (str error))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))

(deftest notification-replays-stay-dismissed-and-deduplication-is-app-and-user-scoped
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-notification-replay-"))
          file (.resolve path directory "dar.sqlite")
          connection (atom (database/open! file))
          store (fn [] (notifications/service
                         #js {:connection @connection :writer (database/writer-queue @connection)}))
          service (atom (store))
          original (atom nil)]
      (.exec @connection "INSERT INTO users(id,subject,display_name,administrator,created_at,updated_at) VALUES
        ('owner','owner','Owner',0,'2026-10-07','2026-10-07'),
        ('reader','reader','Reader',0,'2026-10-07','2026-10-07')")
      (-> (.create @service "owner" "email" "received" {:title "Private mail"} "same-key")
          (.then (fn [created]
                   (reset! original (:id created))
                   (.create @service "owner" "chat" "finished" {:title "Private chat"} "same-key")))
          (.then (fn [created]
                   (is (not= @original (:id created)) "different apps may reuse a deduplication key")
                   (.create @service "reader" "email" "received" {:title "Reader mail"} "same-key")))
          (.then (fn [created]
                   (is (not= @original (:id created)) "the same key belongs independently to each user")
                   (is (= ["Reader mail"] (map #(get-in % [:payload :title]) (.list @service "reader" true))))
                   (-> (.delete @service "reader" @original)
                       (.then (fn [_] (is false "another user's notification cannot be dismissed")))
                       (.catch (fn [error] (is (= :notification-not-found (:type (ex-data error)))))))))
          (.then (fn [_] (.delete @service "owner" @original)))
          (.then (fn [_]
                   (.close @connection)
                   (reset! connection (database/open! file))
                   (reset! service (store))
                   (.create @service "owner" "email" "received" {:title "Replayed mail"} "same-key")))
          (.then (fn [replayed]
                   (is (= @original (:id replayed)) "restart does not discard the dismissed deduplication record")
                   (is (= ["Private chat"] (map #(get-in % [:payload :title]) (.list @service "owner" false))))
                   (is (= 1 (.unreadCount @service "owner")))
                   (is (= 1 (.unreadCount @service "reader")))
                   (.markAllRead @service "owner")))
          (.then (fn [changed]
                   (is (= 1 changed))
                   (is (= [] (.list @service "owner" true)))
                   (is (= 1 (.unreadCount @service "reader")))))
          (.catch (fn [error] (is false (str error))))
          (.finally (fn []
                      (.close @connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))
