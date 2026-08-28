(ns dar.platform.path-test
  (:require [cljs.test :refer [async deftest is testing]]
            [dar.platform.path :as managed-path]
            ["node:fs/promises" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest normalizes-relative-user-paths
  (is (= "projects/dar/readme.md"
         (managed-path/normalize-user-path "projects/dar/readme.md"))))

(deftest rejects-path-escapes-and-reserved-segments
  (doseq [candidate ["" "/etc/passwd" "../secret" "a/../secret"
                     "a//b" "a\\b" ".dar/config" "notes/.trash/file"]]
    (testing candidate
      (try
        (managed-path/normalize-user-path candidate)
        (is false "expected unsafePath")
        (catch js/Error error
          (is (= "unsafePath" (:code (ex-data error)))))))))

(deftest resolves-only-beneath-the-managed-root
  (is (= "/srv/dar/files/documents/report.md"
         (managed-path/resolve-user-path "/srv/dar/files"
                                         "documents/report.md"))))

(deftest rejects-an-existing-symlink-in-the-path-walk
  (async done
    (let [prefix (.resolve path (.tmpdir os) "dar-path-test-")
          temporary (atom nil)]
      (-> (.mkdtemp fs prefix)
          (.then (fn [directory]
                   (reset! temporary directory)
                   (js/Promise.all
                    #js [(.mkdir fs (.resolve path directory "files"))
                         (.mkdir fs (.resolve path directory "outside"))])))
          (.then (fn []
                   (.symlink fs
                             (.resolve path @temporary "outside")
                             (.resolve path @temporary "files" "escape"))))
          (.then (fn []
                   (managed-path/verify-no-symlinks!
                    (.resolve path @temporary "files")
                    "escape/secret.txt")))
          (.then (fn [_]
                   (is false "a symlinked component must be rejected")))
          (.catch (fn [error]
                    (is (= "unsafePath" (:code (ex-data error))))))
          (.finally (fn []
                      (-> (.rm fs @temporary #js {:recursive true :force true})
                          (.finally done))))))))
