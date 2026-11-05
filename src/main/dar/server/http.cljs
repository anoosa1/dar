(ns dar.server.http
  (:require [dar.shared.contracts :as contracts]
            [dar.cordis :as cordis]
            ["node:buffer" :refer [Buffer]]
            ["node:crypto" :as crypto]))

(defn request-id []
  (.randomUUID crypto))

(defn read-json!
  "Reads ordinary API JSON. Syntax failures are client errors; transport and
  cancellation failures retain their original classification."
  [^js request]
  (-> (js/Promise.resolve nil)
      (.then (fn [_] (.json request)))
      (.catch (fn [error]
                (if (instance? js/SyntaxError error)
                  (throw (ex-info "The request body must be valid JSON." {:type :invalid}))
                  (throw error))))))

(defn bounded-json!
  "Large imports own their stream and deadline. Byte limits also apply to
  chunked requests; context disposal cancels a body that has not finished."
  [ctx ^js request byte-limit]
  (when (> (js/Number (.get (.-headers request) "content-length")) byte-limit)
    (throw (ex-info "The import is too large." {:type :too-large})))
  (when-not (.-body request) (throw (ex-info "Choose a JSON import file." {:type :invalid})))
  (let [^js reader (.getReader (.-body request)) chunks (atom []) total (atom 0) stopped (atom nil)
        stop! (fn [reason]
                (compare-and-set! stopped nil reason)
                (.cancel reader))
        dispose! (cordis/effect! ctx "dar.http.import-body"
                   (fn []
                     (let [timer (js/setTimeout #(-> (stop! :unavailable) (.catch (fn [_] nil))) 30000)]
                       (fn [] (js/clearTimeout timer) (stop! :unavailable)))))
        check! #(when @stopped (throw (ex-info "The import was cancelled. Retry when Dar is available." {:type @stopped})))]
    (letfn [(read! []
              (check!)
              (-> (.read reader)
                  (.then (fn [^js result]
                           (check!)
                           (if (.-done result)
                             (try
                               (let [bytes (Buffer.concat (into-array @chunks) @total)
                                     text (.decode (js/TextDecoder. "utf-8" #js {:fatal true}) bytes)]
                                 (js->clj (js/JSON.parse text) :keywordize-keys true))
                               (catch :default _ (throw (ex-info "Choose a valid UTF-8 JSON import file." {:type :invalid}))))
                             (let [chunk (Buffer.from (.-value result))]
                               (swap! total + (.-length chunk))
                               (when (> @total byte-limit) (throw (ex-info "The import is too large." {:type :too-large})))
                               (swap! chunks conj chunk) (read!)))))))]
      (-> (js/Promise.resolve nil) (.then read!)
          (.finally (fn []
                      (reset! chunks [])
                      (-> (js/Promise.resolve (dispose!)) (.catch (fn [_] nil)) (.finally #(.releaseLock reader)))))))))

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
   (error! response status code message nil (request-id)))
  ([response status code message fields]
   (error! response status code message fields (request-id)))
  ([response status code message fields request-id-value]
   (json! response status
          (contracts/error-envelope code message (or request-id-value (request-id)) fields))))
