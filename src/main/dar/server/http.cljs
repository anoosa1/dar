(ns dar.server.http
  (:require [dar.shared.contracts :as contracts]
            ["node:crypto" :as crypto]))

(defn request-id []
  (.randomUUID crypto))

(defn json!
  ([response body]
   (json! response 200 body))
  ([response status body]
   (set! (.-status response) status)
   (.set (.-headers response) "content-type" "application/json; charset=utf-8")
   (.json response (clj->js body))
   (js/Promise.resolve nil)))

(defn text!
  ([response body]
   (text! response 200 body "text/plain; charset=utf-8"))
  ([response status body content-type]
   (set! (.-status response) status)
   (.set (.-headers response) "content-type" content-type)
   (.text response body)
   (js/Promise.resolve nil)))

(defn error!
  ([response status code message]
   (error! response status code message nil))
  ([response status code message fields]
   (json! response status
          (contracts/error-envelope code message (request-id) fields))))

