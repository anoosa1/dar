(ns dar.server.config
  (:require [malli.core :as m]
            ["node:path" :as path]))

(def config-schema
  [:map
   [:host string?]
   [:port [:int {:min 0 :max 65535}]]
   [:baseUrl {:optional true} [:maybe string?]]
   [:dataRoot string?]
   [:publicRoot string?]
   [:installSignals boolean?]])

(defn- parse-port [value]
  (let [parsed (js/Number.parseInt (or value "8080") 10)]
    (if (js/Number.isNaN parsed) -1 parsed)))

(defn from-env
  "Loads runtime configuration from an env-like JavaScript object."
  ([env]
   (from-env env (.cwd js/process)))
  ([env working-directory]
   (let [config {:host (or (aget env "DAR_HOST") "127.0.0.1")
                 :port (parse-port (aget env "DAR_PORT"))
                 :baseUrl (aget env "DAR_BASE_URL")
                 :dataRoot (.resolve path working-directory
                                     (or (aget env "DAR_DATA_ROOT") "data"))
                 :publicRoot (.resolve path working-directory "public")
                 :installSignals true}]
     (when-not (m/validate config-schema config)
       (throw (ex-info "Invalid Dar configuration"
                       {:type :invalid-config
                        :config (dissoc config :baseUrl)})))
     config)))

