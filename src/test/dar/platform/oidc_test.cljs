(ns dar.platform.oidc-test
  (:require [cljs.test :refer [deftest is]]
            [dar.platform.oidc :as oidc]))

(deftest maps-configured-administrator-claims
  (is (true? (oidc/administrator-claim?
              #js {:groups #js ["users" "dar-admins"]}
              "groups" "dar-admins")))
  (is (true? (oidc/administrator-claim?
              #js {:role "administrator"}
              "role" "administrator")))
  (is (false? (oidc/administrator-claim?
               #js {:groups #js ["users"]}
               "groups" "dar-admins")))
  (is (false? (oidc/administrator-claim?
               #js {:role "administrator"}
               nil nil))))
