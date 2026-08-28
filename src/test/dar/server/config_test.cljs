(ns dar.server.config-test
  (:require [cljs.test :refer-macros [deftest is testing]]
            [dar.server.config :as config]))

(deftest loads-safe-defaults
  (let [value (config/from-env #js {} "/tmp/dar-config-test")]
    (is (= "127.0.0.1" (:host value)))
    (is (= 8080 (:port value)))
    (is (= "/tmp/dar-config-test/data" (:dataRoot value)))))

(deftest rejects-invalid-port
  (testing "invalid input never silently selects a different port"
    (is (thrown? cljs.core.ExceptionInfo
                 (config/from-env #js {"DAR_PORT" "wrong"}
                                  "/tmp/dar-config-test")))))

