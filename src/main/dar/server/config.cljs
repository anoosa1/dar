(ns dar.server.config
  (:require [clojure.string :as str]
            [malli.core :as m]
            ["node:path" :as path]))

(def config-schema
  [:map
   [:host string?]
   [:port [:int {:min 0 :max 65535}]]
   [:baseUrl {:optional true} [:maybe string?]]
   [:dataRoot string?]
   [:publicRoot string?]
   [:credentialKeys {:optional true} [:maybe string?]]
   [:bodyLimit [:int {:min 1024 :max 1073741824}]]
   [:uploadBodyLimit [:int {:min 1024 :max 1073741824}]]
   [:quotaBytes [:int {:min 0 :max 9007199254740991}]]
   [:oidc [:maybe
           [:map {:closed true}
            [:issuer string?]
            [:clientId string?]
            [:clientSecret {:optional true} [:maybe string?]]
            [:displayName string?]
            [:adminClaim {:optional true} [:maybe string?]]
            [:adminValue {:optional true} [:maybe string?]]]]]
   [:development boolean?]
   [:installSignals boolean?]])

(defn- parse-port [value]
  (let [parsed (js/Number.parseInt (or value "8080") 10)]
    (if (js/Number.isNaN parsed) -1 parsed)))

(defn- enabled? [value]
  (contains? #{"1" "true" "yes"} (some-> value str/lower-case)))

(defn from-env
  "Loads runtime configuration from an env-like JavaScript object."
  ([env]
   (from-env env (.cwd js/process)))
  ([env working-directory]
   (let [issuer (aget env "DAR_OIDC_ISSUER")
         client-id (aget env "DAR_OIDC_CLIENT_ID")
         client-secret (aget env "DAR_OIDC_CLIENT_SECRET")
         admin-claim (aget env "DAR_OIDC_ADMIN_CLAIM")
         admin-value (aget env "DAR_OIDC_ADMIN_VALUE")
         base-url (aget env "DAR_BASE_URL")
         development (enabled? (aget env "DAR_DEVELOPMENT"))
         oidc-values? (some? (some identity [issuer client-id client-secret
                                             admin-claim admin-value]))
         oidc (when oidc-values?
                {:issuer issuer
                 :clientId client-id
                 :clientSecret client-secret
                 :displayName (or (aget env "DAR_OIDC_DISPLAY_NAME") "OpenID Connect")
                 :adminClaim admin-claim
                 :adminValue admin-value})
         config {:host (or (aget env "DAR_HOST") "127.0.0.1")
                 :port (parse-port (aget env "DAR_PORT"))
                 :baseUrl base-url
                 :dataRoot (.resolve path working-directory
                                     (or (aget env "DAR_DATA_ROOT") "data"))
                 :publicRoot (.resolve path working-directory "public")
                 :credentialKeys (aget env "DAR_CREDENTIAL_ENCRYPTION_KEYS")
                 :bodyLimit (* 1024 1024)
                 :uploadBodyLimit (let [value (aget env "DAR_UPLOAD_BODY_LIMIT")]
                                    (if-not value
                                      (* 128 1024 1024)
                                      (let [parsed (js/Number.parseInt value 10)]
                                        (if (js/Number.isNaN parsed) -1 parsed))))
                 :quotaBytes (let [value (aget env "DAR_FILES_QUOTA_BYTES")]
                               (if-not value
                                 0
                                 (let [parsed (js/Number.parseInt value 10)]
                                   (if (js/Number.isNaN parsed) -1 parsed))))
                 :oidc oidc
                 :development development
                 :installSignals true}]
     (when (or (and oidc-values? (or (str/blank? issuer) (str/blank? client-id)
                                     (str/blank? base-url)))
               (and oidc-values? (not development)
                    (not (str/starts-with? base-url "https://")))
               (not= (some? admin-claim) (some? admin-value)))
       (throw (ex-info "OIDC configuration is incomplete."
                       {:type :invalid-config})))
     (when-not (m/validate config-schema config)
       (throw (ex-info "Invalid Dar configuration"
                       {:type :invalid-config
                        :config (dissoc config :baseUrl :credentialKeys :oidc)})))
     config)))
