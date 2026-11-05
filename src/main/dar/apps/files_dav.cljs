(ns dar.apps.files-dav
  (:require [dar.platform.app-passwords :as app-passwords]
            ["#dar-files/dav" :as dav]))

;; Authentication and Activity belong to the shared platform. Protocol parsing,
;; dispatch, XML, conditions, and file behavior live in TypeScript.
(defn register-dav-routes! [^js server ^js store ^js passwords ^js activity]
  (dav/registerDav
   server (.-typescriptEngine store)
   #js {:authenticate
        (fn [username request]
          (let [credentials (app-passwords/basic-credentials request)]
            (if (or (nil? credentials) (not= username (:username credentials)))
              (js/Promise.resolve "unauthenticated")
              (-> (.verify passwords username (:token credentials))
                  (.then (fn [result]
                           (cond
                             (nil? result) "unauthenticated"
                             (not (app-passwords/has-scope? (:appPassword result) "files")) "forbidden"
                             :else #js {:userId (.-id (:user result))})))))))
        :record
        (fn [user event vpath recipients target-url]
          (if activity
            (.record activity user "files" event
                     {:path vpath :origin "dav"
                      :url target-url}
                     (vec (array-seq recipients)))
            (js/Promise.resolve nil)))
        :error
        (fn [error]
          #js {:type (name (or (:type (ex-data error)) :internal))
               :message (or (ex-message error) "The DAV request failed.")})}))
