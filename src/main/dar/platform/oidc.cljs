(ns dar.platform.oidc
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.platform.auth :as auth]
            [dar.server.http :as http]
            ["openid-client" :as oidc]))

(def flow-cookie "dar-oidc-flow")
(def csrf-cookie "dar-csrf")
(def flow-lifetime-ms (* 10 60 1000))
(def max-flows 1000)

(defn administrator-claim?
  [claims claim-name claim-value]
  (if (and claim-name claim-value)
    (let [value (aget claims claim-name)]
      (if (js/Array.isArray value)
        (.includes value claim-value)
        (= value claim-value)))
    false))

(defn- secure? [base-url]
  (= "https:" (.-protocol (js/URL. base-url))))

(defn- cookie
  [name value {:keys [baseUrl httpOnly maxAge]}]
  (str name "=" value
       "; Path=/; SameSite=Lax"
       (when httpOnly "; HttpOnly")
       (when (secure? baseUrl) "; Secure")
       (when (some? maxAge) (str "; Max-Age=" maxAge))))

(defn- append-cookie! [^js response value]
  ;; The Cordis response adapter flattens WHATWG Headers through Object.fromEntries,
  ;; which would retain only one Set-Cookie value. Preserve the required array on
  ;; Node's ServerResponse until the adapter writes the remaining headers.
  (let [^js raw-response (.-_res response)
        existing (.getHeader raw-response "set-cookie")
        values (cond
                 (nil? existing) #js []
                 (js/Array.isArray existing) existing
                 :else #js [existing])]
    (.setHeader raw-response "set-cookie" (.concat values #js [value]))))

(defn- redirect! [response location]
  (set! (.-status response) 303)
  (.set (.-headers response) "location" location)
  (.text response "")
  (js/Promise.resolve nil))

(defn- clean-flows! [^js flows]
  (let [now (js/Date.now)]
    (doseq [[id ^js flow] flows]
      (when (<= (.-expiresAt flow) now)
        (.delete flows id)))))

(defn- login! [^js oidc-client ^js flows runtime-config response request-id]
  (clean-flows! flows)
  (if (>= (.-size flows) max-flows)
    (http/error! response 429 "rateLimited"
                 "Too many login attempts are in progress." {} request-id)
    (let [verifier (oidc/randomPKCECodeVerifier)
          state (auth/random-token)
          nonce (auth/random-token)
          flow-id (auth/random-token)
          redirect-uri (str (:baseUrl runtime-config) "/api/v1/auth/callback")]
      (-> (oidc/calculatePKCECodeChallenge verifier)
          (.then (fn [challenge]
                   (.set flows flow-id
                         #js {:verifier verifier
                              :state state
                              :nonce nonce
                              :expiresAt (+ (js/Date.now) flow-lifetime-ms)})
                   (append-cookie!
                    response
                    (cookie flow-cookie flow-id
                            {:baseUrl (:baseUrl runtime-config)
                             :httpOnly true
                             :maxAge (quot flow-lifetime-ms 1000)}))
                   (redirect!
                    response
                    (str (oidc/buildAuthorizationUrl
                          oidc-client
                          (clj->js {"redirect_uri" redirect-uri
                                    "scope" "openid profile email"
                                    "code_challenge" challenge
                                    "code_challenge_method" "S256"
                                    "state" state
                                    "nonce" nonce}))))))
          (.catch (fn [_error]
                    (http/error! response 500 "unavailable"
                                 "Login could not be started." {} request-id)))))))

(defn- callback! [^js oidc-client ^js flows ^js auth-service
                  runtime-config request response]
  (let [flow-id (auth/request-cookie request flow-cookie)
        ^js flow (when flow-id (.get flows flow-id))
        request-id (.-requestId request)]
    (when flow-id (.delete flows flow-id))
    (append-cookie! response
                    (cookie flow-cookie ""
                            {:baseUrl (:baseUrl runtime-config)
                             :httpOnly true :maxAge 0}))
    (if (or (nil? flow) (<= (.-expiresAt flow) (js/Date.now)))
      (http/error! response 400 "invalid"
                   "The login attempt is missing or expired." {} request-id)
      (-> (oidc/authorizationCodeGrant
           oidc-client
           (js/URL. (.-url request) (:baseUrl runtime-config))
           #js {:pkceCodeVerifier (.-verifier flow)
                :expectedState (.-state flow)
                :expectedNonce (.-nonce flow)
                :idTokenExpected true})
          (.then (fn [^js tokens]
                   (let [^js claims (.claims tokens)
                         subject (some-> claims .-sub)]
                     (when (str/blank? subject)
                       (throw (ex-info "The ID token has no subject."
                                       {:type :invalid-oidc-claims})))
                     ((.-upsertUser auth-service)
                      #js {:subject subject
                           :displayName (or (.-name claims)
                                            (.-preferred_username claims)
                                            (.-email claims)
                                            subject)
                           :email (.-email claims)
                           :administrator
                           (administrator-claim?
                            claims
                            (get-in runtime-config [:oidc :adminClaim])
                            (get-in runtime-config [:oidc :adminValue]))}))))
          (.then (fn [user]
                   ((.-createSession auth-service) (.-id user))))
          (.then (fn [^js session]
                   (append-cookie!
                    response
                    (cookie auth/session-cookie (.-token session)
                            {:baseUrl (:baseUrl runtime-config)
                             :httpOnly true
                             :maxAge (quot auth/session-lifetime-ms 1000)}))
                   (append-cookie!
                    response
                    (cookie csrf-cookie (.-csrfToken session)
                            {:baseUrl (:baseUrl runtime-config)
                             :httpOnly false
                             :maxAge (quot auth/session-lifetime-ms 1000)}))
                   (redirect! response "/")))
          (.catch (fn [_error]
                    (http/error! response 401 "unauthenticated"
                                 "Login could not be completed." {} request-id)))))))

(defn- register-routes! [^js ctx oidc-client runtime-config]
  (let [^js server (.-server ctx)
        ^js auth-service (.-auth ctx)
        flows (js/Map.)]
    (cordis/effect!
     ctx "dar.oidc.flow-cleanup"
     (fn []
       (let [timer (js/setInterval #(clean-flows! flows) 60000)]
         (fn []
           (js/clearInterval timer)
           (.clear flows)))))
    (.get server "/api/v1/auth/login"
          (fn [^js request response _next]
            (if oidc-client
              (login! oidc-client flows runtime-config response (.-requestId request))
              (http/error! response 503 "unavailable"
                           "OIDC is not configured." {} (.-requestId request)))))
    (.get server "/api/v1/auth/callback"
          (fn [request response _next]
            (if oidc-client
              (callback! oidc-client flows auth-service runtime-config request response)
              (http/error! response 503 "unavailable"
                           "OIDC is not configured." {} (.-requestId request)))))
    (.post server "/api/v1/auth/logout"
           (fn [^js request response _next]
             (let [token (auth/request-cookie request auth/session-cookie)]
               (-> ((.-deleteSession auth-service) token)
                   (.then (fn []
                            (append-cookie!
                             response
                             (cookie auth/session-cookie ""
                                     {:baseUrl (:baseUrl runtime-config)
                                      :httpOnly true :maxAge 0}))
                            (append-cookie!
                             response
                             (cookie csrf-cookie ""
                                     {:baseUrl (:baseUrl runtime-config)
                                      :httpOnly false :maxAge 0}))
                            (http/json! response {:ok true})))))))
    nil))

(def oidc-plugin
  (cordis/plugin
   {:name "dar-oidc"
    :inject ["auth" "server"]
    :apply
    (fn [ctx runtime-config]
      (if-let [{:keys [issuer clientId clientSecret]} (:oidc runtime-config)]
        (let [options (when (:development runtime-config)
                        #js {:execute #js [oidc/allowInsecureRequests]})]
          (-> (oidc/discovery (js/URL. issuer) clientId
                              (or clientSecret js/undefined)
                              js/undefined
                              (or options js/undefined))
            (.then (fn [oidc-client]
                     (register-routes! ctx oidc-client runtime-config)))))
        (register-routes! ctx nil runtime-config)))}))
