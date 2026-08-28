(ns dar.platform.auth-test
  (:require [cljs.test :refer [deftest is]]
            [dar.platform.auth :as auth]
            ["node:buffer" :refer [Buffer]]))

(deftest parses-cookie-headers-without-decoding-token-material
  (is (= {"dar-session" "abc.def_123" "theme" "dark"}
         (auth/parse-cookies "theme=dark; dar-session=abc.def_123")))
  (is (= {} (auth/parse-cookies nil))))

(deftest validates-csrf-tokens-in-constant-time
  (let [token "csrf-token"
        session #js {:csrfHash (auth/token-hash token)}]
    (is (true? (auth/csrf-valid? session token)))
    (is (false? (boolean (auth/csrf-valid? session "wrong"))))
    (is (false? (boolean (auth/csrf-valid? nil token))))))

(deftest generates-url-safe-high-entropy-tokens
  (let [first-token (auth/random-token)
        second-token (auth/random-token)]
    (is (re-matches #"[A-Za-z0-9_-]{43}" first-token))
    (is (not= first-token second-token))
    (is (= 32 (.-length (.from Buffer first-token "base64url"))))))
