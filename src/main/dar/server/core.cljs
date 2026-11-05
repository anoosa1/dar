(ns dar.server.core
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            [dar.shared.contracts :as contracts]
            ["node:fs/promises" :as fs]
            ["node:path" :as path]))

(def spa-paths
  ["/"
   "/login"
   "/files"
   "/contacts"
   "/calendar"
   "/todo"
   "/notes"
   "/news"
   "/email"
   "/activity"
   "/prayer"
   "/weather"
   "/chat"
   "/settings"])

(defn- read-text! [response filename content-type]
  (-> (.readFile fs filename "utf8")
      (.then (fn [body]
               (http/text! response 200 body content-type)))
      (.catch (fn [error]
                (if (= "ENOENT" (.-code error))
                  (http/error! response 404 "notFound" "The requested asset does not exist.")
                  (http/error! response 500 "internal" "The asset could not be read."))))))

(defn- content-type [filename]
  (cond
    (str/ends-with? filename ".js") "text/javascript; charset=utf-8"
    (str/ends-with? filename ".css") "text/css; charset=utf-8"
    (str/ends-with? filename ".svg") "image/svg+xml"
    :else "application/octet-stream"))

(defn- safe-asset [public-root request-path]
  (let [asset-root (.resolve path public-root "assets")
        relative (subs request-path (count "/assets/"))
        target (.resolve path asset-root relative)]
    (when (and (not (str/blank? relative))
               (str/starts-with? target (str asset-root (.-sep path))))
      target)))

(defn- own-http-drain! [ctx ^js server]
  ;; Node's closeIdleConnections only sees sockets idle at that instant. A
  ;; cancelled app request may finish afterwards and become idle during close.
  ;; End those sockets after flushing their response, without truncating it.
  (cordis/effect! ctx "dar.http.response-drain"
    (fn []
      (let [^js listener (.-_http server) stopping (atom false)
            request! (fn [^js request ^js response]
                       (let [finish! #(when @stopping (.end (.-socket request)))]
                         (.once response "finish" finish!)
                         (.once response "close" #(.off response "finish" finish!))))]
        (.on listener "request" request!)
        (fn []
          (reset! stopping true)
          (.off listener "request" request!)
          (.closeIdleConnections listener))))))

(defn- register-routes! [^js ctx {:keys [publicRoot oidc]}]
  (let [^js server (.-server ctx)
        index-file (.resolve path publicRoot "index.html")]
    (own-http-drain! ctx server)
    (.get server "/health"
          (fn [_request response _next]
            (http/json! response
                        {:status "ok"
                         :service "dar"
                         :runtime "node"
                         :apiVersion "1"})))
    (.get server "/api/v1/meta"
          (fn [_request response _next]
            (http/json! response
                        {:apiVersion "1"
                         :name "Dar"
                         :authentication {:oidcConfigured (boolean oidc)
                                          :displayName (or (:displayName oidc)
                                                           "OpenID Connect")}
                         :apps (contracts/collection
                                (mapv (fn [app-path]
                                        {:id (if (= app-path "/") "home" (subs app-path 1))
                                         :path app-path})
                                      spa-paths))})))
    (.get server #"^/assets/.+$"
          (fn [request response _next]
            (if-let [filename (safe-asset publicRoot (.-path request))]
              (-> (.readFile fs filename)
                  (.then (fn [body]
                           (set! (.-status response) 200)
                           (.set (.-headers response) "content-type" (content-type filename))
                           (.set (.-headers response) "cache-control" "no-cache")
                           (.bytes response body)
                           nil))
                  (.catch (fn [error]
                            (if (= "ENOENT" (.-code error))
                              (http/error! response 404 "notFound" "The requested asset does not exist.")
                              (http/error! response 500 "internal" "The asset could not be read.")))))
              (http/error! response 404 "notFound" "The requested asset does not exist."))))
    (doseq [spa-path spa-paths]
      (.get server spa-path
            (fn [_request response _next]
              (read-text! response index-file "text/html; charset=utf-8"))))))

(def core-plugin
  (cordis/plugin
   {:name "dar-core"
    :inject ["server"]
    :apply register-routes!}))
