(ns dar.platform.security
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            ["node:crypto" :refer [randomUUID]]))

(def default-body-limit (* 1024 1024))

(def security-headers
  {"content-security-policy"
   "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
   "cross-origin-opener-policy" "same-origin"
   "cross-origin-resource-policy" "same-origin"
   "referrer-policy" "no-referrer"
   "x-content-type-options" "nosniff"
   "x-frame-options" "DENY"
   "permissions-policy" "camera=(), microphone=(), geolocation=(), payment=()"})

(defn unsafe-method? [method]
  (contains? #{"POST" "PUT" "PATCH" "DELETE"} (str/upper-case method)))

(defn request-origin-allowed?
  "Requires browser mutations under /api/v1 to name Dar's own origin."
  [method request-path origin expected-origin]
  (or (not (and (unsafe-method? method)
                (str/starts-with? request-path "/api/v1")))
      (= origin expected-origin)))

(defn- content-length [^js request]
  (when-let [value (.get (.-headers request) "content-length")]
    (let [parsed (js/Number value)]
      (if (and (js/Number.isSafeInteger parsed) (not (neg? parsed)))
        parsed
        ##Inf))))

(defn- register-security! [^js ctx {:keys [baseUrl bodyLimit]}]
  (let [^js server (.-server ctx)
        expected-origin (.-origin (js/URL. (or baseUrl (.-baseUrl server))))
        limit (or bodyLimit default-body-limit)
        middleware
        (fn [request response next]
          (let [request-id (randomUUID)
                length (content-length request)
                origin (.get (.-headers request) "origin")]
            (set! (.-requestId request) request-id)
            (.set (.-headers response) "x-request-id" request-id)
            (doseq [[header value] security-headers]
              (.set (.-headers response) header value))
            (when (str/starts-with? (.-path request) "/api/")
              (.set (.-headers response) "cache-control" "no-store"))
            (cond
              (and length (> length limit))
              (http/error! response 413 "tooLarge"
                           "The request body is too large." {} request-id)

              (not (request-origin-allowed?
                    (.-method request) (.-path request) origin expected-origin))
              (http/error! response 403 "forbidden"
                           "The request origin is not allowed." {} request-id)

              :else (next))))]
    (cordis/provide! ctx "httpSecurity" #js {:handle middleware})
    nil))

(def security-plugin
  (cordis/plugin
   {:name "dar-http-security"
    :inject ["server"]
    :apply register-security!}))
