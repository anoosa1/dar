(ns dar.platform.security-test
  (:require [cljs.test :refer [deftest is testing]]
            [dar.platform.security :as security]))

(deftest identifies-mutating-methods
  (is (security/unsafe-method? "POST"))
  (is (security/unsafe-method? "delete"))
  (is (not (security/unsafe-method? "GET"))))

(deftest enforces-origin-on-browser-api-mutations
  (let [expected "https://dar.example"]
    (is (security/request-origin-allowed?
         "POST" "/api/v1/files" expected expected))
    (is (not (security/request-origin-allowed?
              "POST" "/api/v1/files" nil expected)))
    (is (not (security/request-origin-allowed?
              "DELETE" "/api/v1/files/1" "https://evil.example" expected)))
    (testing "safe API reads and protocol endpoints do not require Origin"
      (is (security/request-origin-allowed? "GET" "/api/v1/files" nil expected))
      (is (security/request-origin-allowed? "PUT" "/dav/files/user/a" nil expected)))))
