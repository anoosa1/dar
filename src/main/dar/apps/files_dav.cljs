(ns dar.apps.files-dav
  (:require [clojure.string :as str]
            [dar.platform.app-passwords :as app-passwords]
            ["node:crypto" :as crypto]))

;; WebDAV for Files under /dav/files/{username}/{virtual path}. Protocol
;; clients authenticate with scoped Basic app passwords; browser sessions
;; never authorize DAV (routes-and-protocols.md). Responses are standards
;; shaped: 207 multistatus with RFC 4918 depth handling, ETag conditionals,
;; single byte ranges, and XML challenges instead of the /api/v1 envelope.

(defn- xml-escape [value]
  (-> (str value)
      (str/replace "&" "&amp;")
      (str/replace "<" "&lt;")
      (str/replace ">" "&gt;")
      (str/replace "\"" "&quot;")))

(defn- href-escape
  "Percent-encodes each segment so names keep their separators intact."
  [value]
  (str/join "/" (mapv js/encodeURIComponent (str/split value #"/"))))

(defn- dav-etag
  "Cheap strong validator derived from size and mtime (DAV only; the editor
  keeps content-hashed ETags)."
  [entry]
  (str "\"" (:size entry) "-" (.getTime (:updatedAt entry)) "\""))

(defn- dav-target
  "Parses /dav/files/{username}/<vpath> into {:username :vpath} or nil. Each
  segment is decoded once; the engine revalidates every virtual path, so an
  encoded separator or traversal attempt fails closed downstream."
  [request-path]
  (when-let [[_ tail] (re-matches #"/dav/files(?:/(.*))?" request-path)]
    (let [segments (mapv js/decodeURIComponent
                         (remove str/blank? (str/split (or tail "") #"/")))]
      (when (pos? (count segments))
        {:username (first segments)
         :vpath (str "/" (str/join "/" (rest segments)))}))))

(defn- challenge!
  "Sends the standards-appropriate Basic challenge with an XML body."
  [^js response message]
  (set! (.-status response) 401)
  (.set (.-headers response) "www-authenticate"
        "Basic realm=\"dar\", charset=\"UTF-8\"")
  (.set (.-headers response) "content-type" "application/xml; charset=utf-8")
  (.text response
         (str "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
              "<d:error xmlns:d=\"DAV:\"><d:message>"
              (xml-escape message) "</d:message></d:error>"))
  nil)

(defn- dav-error!
  [^js response status message]
  (set! (.-status response) status)
  (.set (.-headers response) "content-type" "application/xml; charset=utf-8")
  (.text response
         (str "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
              "<d:error xmlns:d=\"DAV:\"><d:message>"
              (xml-escape message) "</d:message></d:error>"))
  nil)

(defn- authenticate
  "Resolves Basic credentials for the subject named in the URL. Returns the
  verified user map, :forbidden on a scope mismatch, or :unauthenticated."
  [^js app-passwords username ^js request]
  (let [credentials (app-passwords/basic-credentials request)]
    (cond
      (nil? credentials)
      (js/Promise.resolve :unauthenticated)

      (not= username (:username credentials))
      ;; The URL names the subject; credentials for anyone else fail closed.
      (js/Promise.resolve :unauthenticated)

      :else
      (-> (.verify app-passwords username (:token credentials))
          (.then (fn [result]
                   (cond
                     (nil? result) :unauthenticated
                     (not (app-passwords/has-scope? (:appPassword result) "files"))
                     :forbidden
                     :else (:user result))))))))

(defn- prop-response
  "Builds one <d:response> for an entry; href is the full DAV path."
  [href entry]
  (str "<d:response><d:href>" (xml-escape (href-escape href))
       "</d:href><d:propstat><d:prop>"
       (if (:isDir entry)
         "<d:resourcetype><d:collection/></d:resourcetype>"
         (str "<d:resourcetype/>"
              "<d:getcontentlength>" (:size entry) "</d:getcontentlength>"
              "<d:getetag>" (xml-escape (dav-etag entry)) "</d:getetag>"))
       "<d:getlastmodified>" (.toUTCString (:updatedAt entry)) "</d:getlastmodified>"
       "</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"))

(defn- multistatus!
  [^js response responses]
  (set! (.-status response) 207)
  (.set (.-headers response) "content-type" "application/xml; charset=utf-8")
  (.set (.-headers response) "dav" "1, 2")
  (.text response
         (str "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
              "<d:multistatus xmlns:d=\"DAV:\">"
              (str/join responses)
              "</d:multistatus>"))
  nil)

(defn- depth-of [^js request]
  (let [depth (.get (.-headers request) "depth")]
    (cond
      (str/blank? depth) 1
      (= "0" (str/trim depth)) 0
      (= "1" (str/trim depth)) 1
      :else :infinity)))

(defn- propfind!
  [^js store user-id vpath href ^js request ^js response]
  (let [depth (depth-of request)]
    (if (= :infinity depth)
      (dav-error! response 400 "Depth infinity is not supported.")
      (-> (.stat store user-id vpath)
          (.then (fn [entry]
                   (if (and (= 1 depth) (:isDir entry))
                     ;; .list resolves to the entry collection itself.
                     (-> (.list store user-id vpath)
                         (.then (fn [entries]
                                  (multistatus!
                                   response
                                   (into [(prop-response href entry)]
                                         (map (fn [child]
                                                (prop-response
                                                 (if (= "/" vpath)
                                                   (str href (:name child))
                                                   (str href "/" (:name child)))
                                                 child))
                                              entries))))))
                     (multistatus! response [(prop-response href entry)]))))))))


(defn- html-listing!
  "Directory GET renders a minimal standalone page (baseline parity)."
  [^js response vpath entries]
  (let [base (if (str/ends-with? vpath "/") vpath (str vpath "/"))
        links (mapv (fn [entry]
                      (str "<li><a href=\""
                           (xml-escape (href-escape (str base (:name entry))))
                           "\">" (xml-escape (:name entry)) "</a></li>"))
                    entries)]
    (set! (.-status response) 200)
    (.set (.-headers response) "content-type" "text/html; charset=utf-8")
    (.set (.-headers response) "content-security-policy"
          "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'")
    (.text response
           (str "<!doctype html><html><head><meta charset=\"utf-8\"><title>"
                (xml-escape vpath) "</title></head><body><h1>"
                (xml-escape vpath) "</h1><ul>"
                (str/join links)
                "</ul></body></html>"))
    nil))

(defn- range-of
  "Parses a single byte range against size; nil means the whole body, :bad a
  syntactically valid but unsatisfiable range."
  [^js request size]
  (when-let [header (.get (.-headers request) "range")]
    (if-let [[_ start end] (re-matches #"bytes=(\d*)-(\d*)" (str/trim header))]
      (cond
        (str/blank? start)
        (let [suffix (js/parseInt end 10)]
          (if (and (pos? suffix) (pos? size))
            {:start (max 0 (- size suffix)) :end (dec size)}
            :bad))
        :else
        (let [s (js/parseInt start 10)
              e (if (str/blank? end) (dec size) (min (js/parseInt end 10) (dec size)))]
          (if (and (<= s e) (< s size))
            {:start s :end e}
            :bad)))
      :bad)))

(defn- etag-conditional
  "Checks If-Match/If-None-Match against the entry etag. Returns :ok,
  :precondition (412), or :not-modified (304)."
  [^js request entry]
  (let [etag (dav-etag entry)
        if-match (.get (.-headers request) "if-match")
        if-none-match (.get (.-headers request) "if-none-match")]
    (cond
      (and if-match
           (not (some #(or (= etag %) (= % "*"))
                      (str/split (str/trim if-match) #"\s*,\s*"))))
      :precondition

      (and if-none-match
           (some #(= etag %)
                 (str/split (str/trim if-none-match) #"\s*,\s*")))
      :not-modified

      :else :ok)))

(defn- send-file!
  [^js response ^js bytes entry ^js request head?]
  (let [size (.-length bytes)
        range (range-of request size)]
    (set! (.-status response)
          (cond
            (= range :bad) 416
            range 206
            :else 200))
    (when-not (= range :bad)
      (let [slice (if range (.subarray bytes (:start range) (inc (:end range))) bytes)]
        (when range
          (.set (.-headers response) "content-range"
                (str "bytes " (:start range) "-" (:end range) "/" size)))
        (.set (.-headers response) "content-type" (:mimeType entry))
        (.set (.-headers response) "content-length" (str (.-length slice)))
        (.set (.-headers response) "accept-ranges" "bytes")
        (.set (.-headers response) "etag" (dav-etag entry))
        (.set (.-headers response) "last-modified" (.toUTCString (:updatedAt entry)))
        (if head?
          nil
          (.bytes response slice)))))
  nil)

(defn- get-or-head!
  [^js store user-id vpath ^js request ^js response head?]
  (-> (.stat store user-id vpath)
      (.then (fn [entry]
               (if (:isDir entry)
                 (if head?
                   nil
                   (-> (.list store user-id vpath)
                       (.then (fn [entries]
                                (html-listing! response vpath entries)))))
                 (cond
                   (= :not-modified (etag-conditional request entry))
                   (do (set! (.-status response) 304) nil)

                   :else
                   (-> (.readFile store user-id vpath)
                       (.then (fn [^js bytes]
                                (send-file! response bytes entry request head?))))))))))

(defn- put!
  [^js store user-id vpath ^js request ^js response]
  (-> (.lockInfo store user-id vpath)
      (.then (fn [lock]
               (if (and lock (:locked lock))
                   ;; RFC 4918: writes to a locked resource without its
                   ;; token answer 423. The engine raw save does not
                   ;; enforce locks, so DAV checks explicitly.
                   (dav-error! response 423 "The file is busy.")
                   (-> (.stat store user-id vpath)
                       (.catch (fn [_] nil))
                       (.then (fn [entry]
                                (cond
                                  (and (some? entry)
                                       (= :precondition (etag-conditional request entry)))
                                  (dav-error! response 412 "The resource changed.")

                                  (and (some? entry)
                                       (str/includes? (str (.get (.-headers request) "if-none-match"))
                                                      "*"))
                                  (dav-error! response 412 "The resource already exists.")

                                  :else
                                  (-> (.bytes request)
                                      (.then (fn [^js bytes]
                                               (.save store user-id vpath bytes)))
                                      (.then (fn [_]
                                               (if (some? entry)
                                                 (do (set! (.-status response) 204) nil)
                                                 (do (set! (.-status response) 201) nil))))))))))))))

(defn- destination-target
  "Parses a Destination header into a {:vpath} under the same DAV namespace,
  or nil when it points elsewhere."
  [^js request username]
  (when-let [destination (.get (.-headers request) "destination")]
    (try
      (let [pathname (.-pathname (js/URL. destination))]
        (when-let [target (dav-target pathname)]
          (when (= (:username target) username)
            target)))
      (catch :default _ nil))))

(defn- copy-or-move!
  [^js store user-id username vpath ^js request ^js response move?]
  ;; The Destination subject is the URL name, not the internal id.
  (if-let [target (:vpath (destination-target request username))]
    (-> (if move?
          (.rename store user-id vpath target)
          (.copy store user-id vpath target))
        (.then (fn [_]
                 (set! (.-status response) 201)
                 nil)))
    (dav-error! response 400
                "The Destination header must name a DAV files resource.")))

(defn- lock-token
  "Deterministic DAV lock token for one user and path; the engine lock is a
  per-user advisory write guard without its own token, so the token is
  derived from the pair and verified on UNLOCK."
  [username vpath]
  (str "opaquelocktoken:dar-"
       (-> (.createHash crypto "sha256")
           (.update (str username "\u0000" vpath))
           (.digest "hex"))))

(defn- lock!
  [^js store user-id vpath username ^js request ^js response]
  (let [token (lock-token username vpath)]
    (-> (.lock store user-id vpath)
        (.then (fn [_]
                 (set! (.-status response) 200)
                 (.set (.-headers response) "content-type" "application/xml; charset=utf-8")
                 (.set (.-headers response) "lock-token" (str "<" token ">"))
                 (.text response
                        (str "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
                             "<d:prop xmlns:d=\"DAV:\"><d:lockdiscovery><d:activelock>"
                             "<d:locktype><d:write/></d:locktype>"
                             "<d:lockscope><d:exclusive/></d:lockscope>"
                             "<d:depth>0</d:depth>"
                             "<d:owner>" (xml-escape username) "</d:owner>"
                             "<d:timeout>Second-3600</d:timeout>"
                             "<d:locktoken><d:href>" token "</d:href></d:locktoken>"
                             "<d:lockroot><d:href>"
                             (xml-escape (href-escape (str "/dav/files/" username vpath)))
                             "</d:href></d:lockroot>"
                             "</d:activelock></d:lockdiscovery></d:prop>"))
                 nil)))))

(defn- unlock!
  [^js store user-id vpath username ^js request ^js response]
  (let [header (or (.get (.-headers request) "lock-token")
                   (.get (.-headers request) "if"))
        expected (lock-token username vpath)]
    (if (and header (str/includes? header expected))
      (-> (.unlock store user-id vpath)
          (.then (fn [_]
                   (set! (.-status response) 204)
                   nil)))
      (dav-error! response 409 "The lock token does not match."))))

(defn- dispatch!
  "Runs one authenticated DAV method. Every branch returns a promise or nil;
  the caller maps engine failures onto the XML error shape."
  [^js store user-id username vpath ^js request ^js response method]
  (case method
    "PROPFIND" (propfind! store user-id vpath
                          (str "/dav/files/" username vpath)
                          request response)
    "GET" (get-or-head! store user-id vpath request response false)
    "HEAD" (get-or-head! store user-id vpath request response true)
    "PUT" (put! store user-id vpath request response)
    "DELETE" (-> (.delete store user-id vpath)
                 (.then (fn [_]
                          (set! (.-status response) 204)
                          nil)))
    "MKCOL" (-> (.mkdir store user-id vpath)
                (.then (fn [_]
                         (set! (.-status response) 201)
                         nil)))
    "COPY" (copy-or-move! store user-id username vpath request response false)
    "MOVE" (copy-or-move! store user-id username vpath request response true)
    "LOCK" (lock! store user-id vpath username request response)
    "UNLOCK" (unlock! store user-id vpath username request response)
    (dav-error! response 405 "Method not supported.")))

(defn register-dav-routes!
  [^js server ^js store ^js app-passwords]
  (.all server #"/dav/files.*"
        (fn [request response _next]
          (let [request-path (.-path request)
                method (.-method request)]
            (cond
              (= method "OPTIONS")
              (do (set! (.-status response) 200)
                  (.set (.-headers response) "dav" "1, 2")
                  (.set (.-headers response) "allow"
                        "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, MKCOL, COPY, MOVE, LOCK, UNLOCK")
                  (.set (.-headers response) "content-length" "0")
                  nil)

              (nil? (dav-target request-path))
              (dav-error! response 404 "The requested DAV resource does not exist.")

              :else
              (let [{:keys [username vpath]} (dav-target request-path)]
                (-> (authenticate app-passwords username request)
                    (.then (fn [auth]
                             (cond
                               (= auth :unauthenticated)
                               (challenge! response "Authentication is required.")

                               (= auth :forbidden)
                               (dav-error! response 403
                                           "This app password cannot access files.")

                               :else
                               (dispatch! store (.-id auth) username vpath
                                          request response method))))
                    (.catch (fn [error]
                              (let [type (:type (ex-data error))]
                                (case type
                                  :not-found (dav-error! response 404 (ex-message error))
                                  :invalid (dav-error! response 400 (ex-message error))
                                  :conflict (dav-error! response 409 (ex-message error))
                                  :already-exists (dav-error! response 409 (ex-message error))
                                  :locked (dav-error! response 423 (ex-message error))
                                  :too-large (dav-error! response 413 (ex-message error))
                                  :quota-exceeded (dav-error! response 507 (ex-message error))
                                  :forbidden (dav-error! response 403 (ex-message error))
                                  (do (js/console.error
                                       "dav request failed"
                                       (ex-message error))
                                      (dav-error! response 500
                                                  "The DAV request failed.")))))))))))))
