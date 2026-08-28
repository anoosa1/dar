(ns dar.platform.search-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.database :as database]
            [dar.platform.search :as search]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest indexes-updates-queries-and-deletes-scoped-documents
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-search-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (search/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES
              ('user-1', 'subject-1', 'User One', 'one@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'),
              ('user-2', 'subject-2', 'User Two', 'two@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.index service "user-1" "files" "file-1"
                  "Quarterly report" "Revenue and planning" "/files/quarter.pdf")
          (.then (fn [created]
                   (is (= 1 (:generation created)))
                   (.index service "user-1" "files" "file-1"
                           "Quarterly report" "Revenue and planning changed"
                           "/files/quarter.pdf")))
          (.then (fn [updated]
                   (is (= 2 (:generation updated)))
                   (.index service "user-2" "files" "file-1"
                           "Private report" "Another user's document" "/files/private.pdf")))
          (.then (fn [_]
                   (is (= 1 (count (.search service "user-1" "quarter"))))
                   (is (= 1 (count (.search service "user-2" "private"))))
                   (is (= 0 (count (.search service "user-1" "private"))))
                   (.delete service "user-1" "files" "file-1")))
          (.then (fn [_]
                   (is (= 0 (count (.search service "user-1" "quarter"))))))
          (.catch (fn [error]
                    (is false (str error))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))
