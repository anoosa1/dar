(ns dar.shared.contracts-test
  (:require [cljs.test :refer-macros [deftest is testing]]
            [dar.shared.contracts :as contracts]
            [malli.core :as m]))

(deftest stable-error-envelope
  (testing "known errors satisfy the public envelope"
    (let [value (contracts/error-envelope
                 "conflict"
                 "The resource changed."
                 "request-1"
                 {:title "Required"})]
      (is (contracts/valid-error-envelope? value))
      (is (= "conflict" (get-in value [:error :code])))))
  (testing "collection responses use cursor pagination"
    (is (m/validate contracts/collection-schema
                    (contracts/collection [{:id "one"}] "next")))))

