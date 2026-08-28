(ns dar.platform.keyring-test
  (:require [cljs.test :refer [deftest is]]
            [dar.platform.keyring :as keyring]
            ["node:buffer" :refer [Buffer]]))

(defn- encoded-key [fill]
  (.toString (.alloc Buffer 32 fill) "base64"))

(deftest encrypts-authenticates-and-rotates-credentials
  (let [service (keyring/keyring
                 (keyring/parse-keys
                  (str "new:" (encoded-key 1) ",old:" (encoded-key 2))))
        encrypted ((.-encrypt service) "secret value")]
    (is (= "secret value" ((.-decrypt service) encrypted)))
    (is (false? ((.-needsRotation service) encrypted)))
    (let [old-service (keyring/keyring
                       (keyring/parse-keys (str "old:" (encoded-key 2))))
          old-value ((.-encrypt old-service) "rotate me")]
      (is (= "rotate me" ((.-decrypt service) old-value)))
      (is (true? ((.-needsRotation service) old-value))))))

(deftest rejects-invalid-keys-and-tampering
  (is (thrown? js/Error (keyring/parse-keys "bad:bm90LTMyLWJ5dGVz")))
  (let [service (keyring/keyring
                 (keyring/parse-keys (str "main:" (encoded-key 3))))
        encrypted ((.-encrypt service) "secret")
        tampered (str encrypted "x")]
    (is (thrown? js/Error ((.-decrypt service) tampered)))))

(deftest unavailable-keyring-fails-closed
  (let [service (keyring/keyring [])]
    (is (false? (.-available service)))
    (is (thrown? js/Error ((.-encrypt service) "secret")))))
