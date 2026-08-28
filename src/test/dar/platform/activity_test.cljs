(ns dar.platform.activity-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.activity :as activity]
            [dar.platform.database :as database]
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
                {:path "reports/quarter.pdf"} ["user-1" "user-2"])
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
