(ns dar.platform.keyring
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            ["node:buffer" :refer [Buffer]]
            ["node:crypto" :as crypto]))

(def key-bytes 32)

(defn- decode-key [encoded]
  (let [^js material (.from Buffer encoded "base64")]
    (when-not (= key-bytes (.-length material))
      (throw (ex-info "Credential keys must decode to exactly 32 bytes."
                      {:type :invalid-credential-key})))
    material))

(defn parse-keys
  "Parses ordered `id:base64` key material. The first key is active."
  [value]
  (if (str/blank? value)
    []
    (mapv
     (fn [entry]
       (let [[id encoded extra] (str/split entry #":" 3)]
         (when (or (str/blank? id) (str/blank? encoded) extra
                   (not (re-matches #"[A-Za-z0-9._-]+" id)))
           (throw (ex-info "Credential keys must use id:base64 entries."
                           {:type :invalid-credential-key})))
         {:id id :material (decode-key encoded)}))
     (str/split value #","))))

(defn keyring [keys]
  (let [key-map (into {} (map (juxt :id :material) keys))
        active (first keys)]
    #js {:available (boolean active)
         :activeKeyId (:id active)
         :encrypt
         (fn [plaintext]
           (when-not active
             (throw (ex-info "Credential encryption is not configured."
                             {:type :credential-keyring-unavailable})))
           (let [iv (crypto/randomBytes 12)
                 cipher (crypto/createCipheriv "aes-256-gcm" (:material active) iv)
                 ciphertext (.concat Buffer
                                     #js [(.update cipher plaintext "utf8")
                                          (.final cipher)])
                 tag (.getAuthTag cipher)]
             (str "v1:" (:id active) ":"
                  (.toString iv "base64url") ":"
                  (.toString ciphertext "base64url") ":"
                  (.toString tag "base64url"))))
         :decrypt
         (fn [encoded]
           (let [[version key-id iv-text ciphertext-text tag-text extra]
                 (str/split encoded #":" 6)
                 material (get key-map key-id)]
             (when (or (not= version "v1") extra
                       (some str/blank? [key-id iv-text ciphertext-text tag-text]))
               (throw (ex-info "The encrypted credential has an unsupported format."
                               {:type :invalid-encrypted-credential})))
             (when-not material
               (throw (ex-info "The credential key is unavailable."
                               {:type :missing-credential-key :keyId key-id})))
             (try
               (let [decipher (crypto/createDecipheriv
                               "aes-256-gcm" material
                               (.from Buffer iv-text "base64url"))]
                 (.setAuthTag decipher (.from Buffer tag-text "base64url"))
                 (.toString
                  (.concat Buffer
                           #js [(.update decipher
                                         (.from Buffer ciphertext-text "base64url"))
                                (.final decipher)])
                  "utf8"))
               (catch :default error
                 (throw (ex-info "The encrypted credential could not be authenticated."
                                 {:type :invalid-encrypted-credential}
                                 error))))))
         :needsRotation
         (fn [encoded]
           (not= (:id active) (second (str/split encoded #":" 3))))
         :mac
         ;; HMAC-SHA256 under the active credential key. The domain prefix
         ;; separates protocols sharing the key (share passwords, share
         ;; access cookies). Returns a Buffer for timing-safe comparisons.
         (fn [domain input]
           (when-not active
             (throw (ex-info "Credential keys are not configured."
                             {:type :credential-keyring-unavailable})))
           (let [hmac (crypto/createHmac "sha256" (:material active))]
             (.update hmac (str domain "\u0000"))
             (.update hmac input)
             (.digest hmac)))}))

(def keyring-plugin
  (cordis/plugin
   {:name "dar-keyring"
    :apply (fn [ctx {:keys [credentialKeys]}]
             (cordis/provide! ctx "keyring" (keyring (parse-keys credentialKeys)))
             nil)}))
