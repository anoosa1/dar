(ns dar.platform.contributions-test
  (:require [cljs.test :refer [deftest is]]
            [dar.platform.contributions :as contributions]))

(deftest registers-lists-and-disposes-contributions
  (let [^js registry (contributions/registry)
        dispose ((.-register registry) "widgets" "example.clock" #js {:size "small"})]
    (is (= ["example.clock"] (mapv (fn [[id _]] id) (.list registry "widgets"))))
    (dispose)
    (is (= [] (mapv (fn [[id _]] id) (.list registry "widgets"))))))

(deftest rejects-invalid-or-duplicate-contributions
  (let [^js registry (contributions/registry)]
    (is (thrown? js/Error ((.-register registry) "unknown.kind" "example.valid" #js {})))
    (is (thrown? js/Error ((.-register registry) "widgets" "clock" #js {})))
    ((.-register registry) "widgets" "example.clock" #js {})
    (is (thrown? js/Error ((.-register registry) "widgets" "example.clock" #js {})))))
