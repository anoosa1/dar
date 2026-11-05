(ns dar.platform.assistant
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            [malli.core :as m]
            [malli.error :as me]
            [malli.json-schema :as malli.json-schema]))

;; Typed, user-scoped assistant tools. Apps register tool descriptors through
;; the contributions registry; disposal of the owning context removes them.
;; The framework owns validation, effect classification, confirmation gating,
;; and stable non-secret errors. Tools themselves never touch HTTP.

(def effect-kinds
  #{"read" "safeWrite" "external" "destructive"})

(def tool-id-pattern
  #"[a-z][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)+")

;; Stable error categories a tool may throw as ex-info :type values.
(def tool-error-mapping
  {:invalid [400 "invalid"]
   :not-found [404 "notFound"]
   :forbidden [403 "forbidden"]
   :readonly [403 "forbidden"]
   :conflict [409 "conflict"]
   :rate-limited [429 "rateLimited"]
   :too-large [413 "tooLarge"]
   :precondition-required [428 "preconditionRequired"]
   :precondition-failed [412 "preconditionFailed"]
   :preference-conflict [412 "preconditionFailed"]
   :preference-not-found [412 "preconditionFailed"]
   :unavailable [503 "unavailable"]})

(defn- valid-descriptor? [descriptor]
  (and (map? descriptor)
       (string? (:name descriptor))
       (re-matches tool-id-pattern (:name descriptor))
       (string? (:title descriptor))
       (not (str/blank? (:title descriptor)))
       (contains? effect-kinds (:effect descriptor))
       (fn? (:invoke descriptor))
       (some? (:schema descriptor))))

(defn register-tool!
  "Registers one validated tool descriptor under kind assistantTools."
  [^js registry id descriptor]
  (let [descriptor (if (map? descriptor) descriptor {})]
    (when-not (and (string? id) (re-matches tool-id-pattern id))
      (throw (ex-info "Tool ids must be dotted and namespaced."
                      {:type :invalid-tool :id id})))
    (when-not (valid-descriptor? descriptor)
      (throw (ex-info "A tool needs a namespaced name, title, effect, schema, and invoke function."
                      {:type :invalid-tool :id id})))
    (when-not (= id (:name descriptor))
      (throw (ex-info "The tool name must match its contribution id."
                      {:type :invalid-tool :id id})))
    ;; Compiling the schema here rejects malformed descriptors at registration
    ;; time instead of invocation time.
    (m/schema (:schema descriptor))
    (.register registry "assistantTools" id (assoc descriptor :name id))))

(defn- registry-entries [^js registry kind]
  (let [iterator (.list registry kind)]
    (loop [acc []]
      (let [step (.next iterator)]
        (if (.-done step)
          acc
          (let [[id value] (js->clj (.-value step))]
            (recur (conj acc [id value]))))))))

(defn list-tools
  "Public tool catalog: name, title, effect, and a JSON Schema of arguments."
  [^js registry]
  (let [items (->> (registry-entries registry "assistantTools")
                   (sort-by first)
                   (map (fn [[_id descriptor]]
                          {:name (:name descriptor)
                           :title (:title descriptor)
                           :effect (:effect descriptor)
                           :parameters (malli.json-schema/transform
                                        (:schema descriptor))})))]
    (vec items)))

(defn- require-confirmation? [descriptor]
  (contains? #{"external" "destructive"} (:effect descriptor)))

(defn invoke
  "Validates arguments and runs one tool. Returns a promise of the result.

  Failures resolve to {:error ...} records so HTTP stays in this layer."
  [^js registry user tool-id arguments confirm]
  (let [tool (->> (registry-entries registry "assistantTools")
                  (some (fn [[id descriptor]]
                          (when (= id tool-id) descriptor))))]
    (cond
      (nil? tool)
      (js/Promise.resolve {:error :tool-not-found})

      (and (require-confirmation? tool) (not (true? confirm)))
      (js/Promise.resolve {:error :confirmation-required :effect (:effect tool)})

      :else
      (let [schema (:schema tool)
            explanation (m/explain schema arguments)]
        (if (nil? explanation)
          (-> (js/Promise.resolve nil)
              (.then (fn [_] ((:invoke tool) user arguments)))
              (.then (fn [result] {:result result}))
              (.catch (fn [error] {:error :tool-failed :cause error})))
          (js/Promise.resolve
           {:error :invalid-arguments
            :fields (me/humanize explanation)}))))))

(defn- require-session! [^js request response]
  (if-let [user (.-user request)]
    user
    (do (http/error! response 401 "unauthenticated"
                     "Authentication is required." {} (.-requestId response))
        nil)))

(defn- send-invocation-error! [^js response outcome]
  (let [request-id (.-requestId response)]
    (case (:error outcome)
      :tool-not-found
      (http/error! response 404 "notFound"
                   "No such assistant tool." {} request-id)
      :confirmation-required
      (http/error! response 409 "conflict"
                   "This tool requires explicit confirmation." {} request-id)
      :invalid-arguments
      (http/error! response 400 "invalid"
                   "The tool arguments are invalid."
                   (:fields outcome)
                   request-id)
      (http/error! response 500 "internal"
                   "The tool failed." {} request-id))))

(defn- send-tool-failure! [^js response ^js error]
  (let [request-id (.-requestId response)
        category (:type (ex-data error))
        [status code] (get tool-error-mapping category)]
    (if (some? status)
      (http/error! response status code (ex-message error) {} request-id)
      ;; Unknown categories stay generic so tool internals never leak.
      (http/error! response 500 "internal" "The tool failed." {} request-id))))

(defn- register-api! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js registry (.-contributions ctx)]
    (.get server "/api/v1/assistant/tools"
          (fn [^js request response _next]
            (when (require-session! request response)
              (http/json! response {:items (list-tools registry)}))))
    (.post server "/api/v1/assistant/tools/:id/invoke"
           (fn [^js request response _next]
             (when (require-session! request response)
               (-> (http/read-json! request)
                   (.then (fn [body]
                            (let [value (js->clj body :keywordize-keys true)]
                              (invoke registry
                                      (.-user request)
                                      (.-id (.-params request))
                                      (:arguments value)
                                      (:confirm value)))))
                   (.then (fn [outcome]
                            (if-let [failure (:error outcome)]
                              (if (= :tool-failed failure)
                                (send-tool-failure! response (:cause outcome))
                                (send-invocation-error! response outcome))
                              (http/json! response (:result outcome)))))
                   (.catch (fn [error] (send-tool-failure! response error)))))))
    nil))

(def assistant-plugin
  (cordis/plugin
   {:name "dar-assistant"
    :inject ["contributions" "server"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "assistant" #js {:registerTool (partial register-tool! (.-contributions ctx))
                                            :listTools (partial list-tools (.-contributions ctx))
                                            :invoke (partial invoke (.-contributions ctx))})
      (register-api! ctx _config))}))
