(ns dar.platform.notifications-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.database :as database]
            [dar.platform.notifications :as notifications]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

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
