(ns dar.platform.instance-test
  (:require [cljs.test :refer [async deftest is testing]]
            [dar.platform.instance :as instance]
            ["node:fs/promises" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest validates-current-instance-marker
  (is (= {:product "dar"
          :dataVersion 1
          :instanceId "instance-1"
          :database "dar.sqlite"}
         (instance/validate-instance
          #js {:product "dar"
               :dataVersion 1
               :instanceId "instance-1"
               :database "dar.sqlite"}))))

(deftest rejects-foreign-and-future-data-roots
  (testing "a legacy or unrelated marker is rejected"
    (is (thrown? js/Error
                 (instance/validate-instance
                  #js {:product "other"
                       :dataVersion 1
                       :instanceId "instance-1"
                       :database "dar.sqlite"}))))
  (testing "future versions fail closed"
    (is (thrown? js/Error
                 (instance/validate-instance
                  #js {:product "dar"
                       :dataVersion 2
                       :instanceId "instance-1"
                       :database "dar.sqlite"})))))

(deftest creates-and-reopens-a-fresh-data-root
  (async done
    (let [root (.resolve path (.tmpdir os) "dar-instance-test-")
          temporary (atom nil)]
      (-> (.mkdtemp fs root)
          (.then (fn [directory]
                   (reset! temporary directory)
                   (instance/ensure! directory)))
          (.then (fn [created]
                   (is (= 1 (:dataVersion created)))
                   (is (string? (:instanceId created)))
                   (instance/ensure! @temporary)))
          (.then (fn [reopened]
                   (is (= "dar.sqlite" (:database reopened)))
                   (is (= (:dataRoot reopened) @temporary))))
          (.catch (fn [error]
                    (is false (str "fresh instance failed: " (.-message error)))))
          (.finally (fn []
                      (-> (.rm fs @temporary #js {:recursive true :force true})
                          (.finally done))))))))

(deftest rejects-an-unmarked-non-empty-data-root
  (async done
    (let [root (.resolve path (.tmpdir os) "dar-foreign-root-test-")
          temporary (atom nil)]
      (-> (.mkdtemp fs root)
          (.then (fn [directory]
                   (reset! temporary directory)
                   (.writeFile fs (.resolve path directory "dar.db") "legacy")))
          (.then (fn []
                   (instance/ensure! @temporary)))
          (.then (fn [_]
                   (is false "an unmarked non-empty root must be rejected")))
          (.catch (fn [error]
                    (is (= :unrecognized-data-root (:type (ex-data error))))))
          (.finally (fn []
                      (-> (.rm fs @temporary #js {:recursive true :force true})
                          (.finally done))))))))
