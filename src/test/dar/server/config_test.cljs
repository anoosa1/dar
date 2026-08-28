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

(deftest loads-complete-oidc-configuration
  (let [value (config/from-env
               #js {"DAR_BASE_URL" "https://dar.example"
                    "DAR_OIDC_ISSUER" "https://identity.example"
                    "DAR_OIDC_CLIENT_ID" "dar"
                    "DAR_OIDC_CLIENT_SECRET" "secret"
                    "DAR_OIDC_ADMIN_CLAIM" "groups"
                    "DAR_OIDC_ADMIN_VALUE" "dar-admins"}
               "/tmp/dar-config-test")]
    (is (= "https://identity.example" (get-in value [:oidc :issuer])))
    (is (= "groups" (get-in value [:oidc :adminClaim])))))

(deftest rejects-partial-oidc-configuration
  (is (thrown? cljs.core.ExceptionInfo
               (config/from-env #js {"DAR_OIDC_ISSUER" "https://identity.example"}
                                "/tmp/dar-config-test")))
  (is (thrown? cljs.core.ExceptionInfo
               (config/from-env #js {"DAR_OIDC_ISSUER" "https://identity.example"
                                     "DAR_OIDC_CLIENT_ID" "dar"
                                     "DAR_OIDC_ADMIN_CLAIM" "groups"}
                                "/tmp/dar-config-test"))))
