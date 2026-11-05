(ns dar.platform.dashboard
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            [dar.shared.revisions :as revisions]
            [malli.core :as m]))

;; Dashboard catalog and per-user layout. Widget definitions arrive as widgets
;; contributions; the layout is one JSON document stored under preferences
;; (namespace "dashboard", key "layout") exactly like the baseline: a
;; twelve-column grid with semantic sizes, validation, and reflow on save.

(def columns 12)
(def max-y 10000)
(def layout-namespace "dashboard")
(def layout-key "layout")

(def presets
  [{:size "compact" :label "Compact" :w 3 :h 2 :mobileH 2}
   {:size "standard" :label "Standard" :w 4 :h 3 :mobileH 4}
   {:size "wide" :label "Wide" :w 6 :h 4 :mobileH 6}
   {:size "large" :label "Large" :w 8 :h 6 :mobileH 8}])

(defn preset-of [size]
  (some (fn [preset] (when (= size (:size preset)) preset)) presets))

(def layout-schema
  [:map {:closed true}
   [:layout
    [:vector
     [:map {:closed true}
      [:id string?]
      [:x int?]
      [:y int?]
      [:size string?]]]]
   [:activeId {:optional true} string?]])

(defn- widget-id-pattern [] #"[a-z][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)+")

(defn register-widget!
  "Registers one validated widget definition under kind widgets."
  [^js registry id descriptor]
  (let [descriptor (if (map? descriptor) descriptor {})
        sizes (:supported-sizes descriptor)]
    (when-not (and (string? id) (re-matches (widget-id-pattern) id))
      (throw (ex-info "Widget ids must be dotted and namespaced."
                      {:type :invalid-widget :id id})))
    (when-not (and (string? (:title descriptor))
                   (not (str/blank? (:title descriptor)))
                   (string? (:kind descriptor))
                   (contains? (set (map :size presets)) (:default-size descriptor))
                   (vector? sizes)
                   (pos? (count sizes))
                   (every? #(contains? (set (map :size presets)) %) sizes)
                   (fn? (:fetch descriptor)))
      (throw (ex-info "A widget needs a title, kind, supported sizes, and a data fetch function."
                      {:type :invalid-widget :id id})))
    (when-not (= id (:id descriptor))
      (throw (ex-info "The widget id must match its contribution id."
                      {:type :invalid-widget :id id})))
    (.register registry "widgets" id
               (assoc descriptor :id id :supported-sizes (vec sizes)))))

(defn catalog
  "The widget catalog as stored in the registry, in registration order."
  [^js registry]
  (let [iterator (.list registry "widgets")]
    (loop [acc []]
      (let [step (.next iterator)]
        (if (.-done step)
          acc
          ;; The entry pair is [id, descriptor] where the descriptor is already
          ;; CLJS data; cloning it through js->clj would mangle keyword keys.
          (let [entry (.-value step)
                descriptor (aget entry 1)]
            (recur (conj acc
                         {:id (:id descriptor)
                          :title (:title descriptor)
                          :icon (:icon descriptor)
                          :kind (:kind descriptor)
                          :defaultSize (:default-size descriptor)
                          :supportedSizes (:supported-sizes descriptor)
                          :emptyState (:empty-state descriptor)
                          :actions (:actions descriptor)
                          :configurable (:configurable descriptor)}))))))))

(defn- definitions-by-id [widgets]
  (into {} (map (fn [widget] [(:id widget) widget])) widgets))

(defn- raw-widgets
  "The registered widget descriptors as stored, including the data and
  settings handler functions that the public catalog record omits."
  [^js registry]
  (let [iterator (.list registry "widgets")]
    (loop [acc []]
      (let [step (.next iterator)]
        (if (.-done step)
          acc
          (recur (conj acc (aget (.-value step) 1))))))))

(defn- supports-size? [definition size]
  (contains? (set (:supportedSizes definition)) size))

(defn- sort-items [items]
  (sort-by (fn [item] [(:y item) (:x item)]) items))

(defn- collides? [item placed definitions]
  (let [preset (preset-of (:size item))]
    (if (or (nil? preset)
            (not (supports-size? (get definitions (:id item)) (:size item))))
      true
      (some (fn [other]
              (let [other-preset (preset-of (:size other))]
                (and (some? other-preset)
                     (< (:x item) (+ (:x other) (:w other-preset)))
                     (< (:x other) (+ (:x item) (:w preset)))
                     (< (:y item) (+ (:y other) (:h other-preset)))
                     (< (:y other) (+ (:y item) (:h preset))))))
            placed))))

(defn- reflow
  "Packs the layout: sorted by position, collisions push items down, and
  compaction then moves items up into freed space."
  [layout definitions]
  (let [items (sort-items (:layout layout))
        placed (reduce
                (fn [acc item]
                  (let [preset (preset-of (:size item))]
                    (if (or (nil? preset)
                            (not (supports-size? (get definitions (:id item)) (:size item))))
                      acc
                      (let [item (assoc item
                                        :x (min (max (if (int? (:x item)) (:x item) 0) 0)
                                                (- columns (:w preset)))
                                        :y (min max-y (max (if (int? (:y item)) (:y item) 0) 0)))
                            item (loop [candidate item]
                                   (if (collides? candidate acc definitions)
                                     (recur (update candidate :y inc))
                                     candidate))]
                        (conj acc item)))))
                []
                items)
        ;; Each item sees the already compacted positions above it. Comparing
        ;; against the old positions leaves gaps that shrink again on every GET.
        compacted
        (reduce
         (fn [acc item]
           (conj acc
             (loop [candidate item]
               (let [moved (update candidate :y dec)]
                 (if (and (pos? (:y candidate)) (not (collides? moved acc definitions)))
                   (recur moved)
                   candidate)))))
         [] (sort-items placed))]
    (assoc layout :layout (vec (sort-items compacted)) :activeId "")))

(defn default-layout
  "Auto-packs every catalog widget at its default size, left to right."
  [widgets]
  (let [all-widgets (vec widgets)]
    (loop [widgets all-widgets
           x 0
           y 0
           row-height 0
           layout []]
      (if-let [widget (first widgets)]
        (let [preset (preset-of (:defaultSize widget))
              width (or (:w preset) 0)
              height (or (:h preset) 0)
              wrap? (> (+ x width) columns)
              x (if wrap? 0 x)
              y (if wrap? (+ y row-height) y)
              row-height (if wrap? height (max row-height height))]
          (recur (rest widgets)
                 (+ x width)
                 y
                 row-height
                 (conj layout
                       {:id (:id widget)
                        :x x
                        :y y
                        :size (if (supports-size? widget (:defaultSize widget))
                                (:defaultSize widget)
                                (first (:supportedSizes widget)))})))
        (reflow {:layout layout :activeId ""} (definitions-by-id all-widgets))))))

(defn- valid-item? [item definitions]
  (and (contains? definitions (:id item))
       (supports-size? (get definitions (:id item)) (:size item))
       (int? (:x item)) (>= (:x item) 0) (< (:x item) columns)
       (int? (:y item)) (>= (:y item) 0) (<= (:y item) max-y)))

(defn validate-layout
  "Returns nil when the layout is acceptable, else a stable error keyword."
  [layout definitions]
  (let [ids (map :id (:layout layout))
        active-id (:activeId layout)]
    (cond
      (some #(not (contains? definitions %)) ids) :unknown-widget
      (not= (count ids) (count (distinct ids))) :duplicate-widget
      (some #(not (supports-size? (get definitions (:id %)) (:size %)))
            (:layout layout)) :unsupported-size
      (some #(or (not (int? (:x %))) (< (:x %) 0) (>= (:x %) columns)
                 (not (int? (:y %))) (< (:y %) 0) (> (:y %) max-y))
            (:layout layout)) :invalid-coordinates
      (and (some? active-id) (not= active-id "")
           (not (contains? (set ids) active-id))) :active-widget-missing)))

(defn normalize-layout
  "Drops unknown and duplicate widgets, clamps coordinates, and reflows.
  Returns [layout changed?]."
  [layout definitions]
  (let [original (when (map? layout) (:layout layout))
        candidates (if (vector? original) original (:layout (default-layout (vals definitions))))
        seen (atom #{})
        kept (filter (fn [item]
                       (when (and (map? item) (contains? definitions (:id item))
                                  (not (contains? @seen (:id item))))
                         (swap! seen conj (:id item))
                         item))
                     candidates)
        fixed (map (fn [item]
                     (let [definition (get definitions (:id item))
                           size (if (supports-size? definition (:size item))
                                  (:size item)
                                  (if (supports-size? definition (:defaultSize definition)) (:defaultSize definition) (first (:supportedSizes definition))))]
                       ;; Generic preference writes and older documents can
                       ;; bypass layout validation. Bound geometry before sorting
                       ;; or reflow so malformed values cannot stall the host.
                       (assoc item :size size
                              :x (if (int? (:x item)) (min (dec columns) (max 0 (:x item))) 0)
                              :y (if (int? (:y item)) (min max-y (max 0 (:y item))) 0))))
                   kept)
        normalized (reflow {:layout (vec fixed) :activeId ""}
                           definitions)]
    [normalized (not= original (:layout normalized))]))

(def error-mapping
  {:invalid [400 "invalid"]
   :invalid-body [400 "invalid"]
   :invalid-timezone [400 "invalid"]
   :invalid-settings [400 "invalid"]
   :unknown-widget [400 "invalid"]
   :duplicate-widget [400 "invalid"]
   :unsupported-size [400 "invalid"]
   :invalid-coordinates [400 "invalid"]
   :active-widget-missing [400 "invalid"]
   :not-found [404 "notFound"]
   :forbidden [403 "forbidden"]
   :conflict [409 "conflict"]
   :rate-limited [429 "rateLimited"]
   :unavailable [503 "unavailable"]})

(defn- valid-timezone? [value]
  (try
    (js/Intl.DateTimeFormat. "en" #js {:timeZone value})
    true
    (catch :default _ false)))

(defn service
  "Builds the dashboard service over preferences and the widget registry."
  [^js preferences ^js registry]
  (let [definitions (fn [] (definitions-by-id (catalog registry)))
        handlers (fn [] (definitions-by-id (raw-widgets registry)))
        widgets (fn [] (catalog registry))
        load-layout
        (fn [user-id]
          (let [stored (.get preferences user-id layout-namespace layout-key)
                [layout changed] (normalize-layout
                                  (or (:value stored)
                                      (default-layout (widgets)))
                                  (definitions))]
            (if (and changed (nil? stored))
              (.then (.set preferences user-id layout-namespace layout-key layout nil)
                     (fn [saved] (assoc layout :revision (:revision saved))))
              (if changed
                (.then (.set preferences user-id layout-namespace layout-key layout
                             (:revision stored))
                       (fn [saved] (assoc layout :revision (:revision saved))))
                (js/Promise.resolve (assoc layout :revision (or (:revision stored) 0)))))))
        save-layout
        (fn [user-id layout expected-revision]
          (let [error (validate-layout layout (definitions))]
            (if (some? error)
              (js/Promise.reject
               (ex-info "The dashboard layout is invalid."
                        {:type :invalid-layout :error error}))
              (let [reflowed (reflow layout (definitions))
                    current (.get preferences user-id layout-namespace layout-key)]
                (revisions/check! expected-revision (or (:revision current) 0))
                (.then (.set preferences user-id layout-namespace layout-key
                             reflowed (:revision current))
                       (fn [saved] (assoc reflowed :revision (:revision saved))))))))
        widget-not-found!
        (fn [widget-id]
          (js/Promise.reject
           (ex-info "Widget not found." {:type :not-found :id widget-id})))]
    #js {:catalog (fn [] {:presets presets :widgets (widgets)})
         :loadLayout load-layout
         :saveLayout save-layout
         :widgetData
         (fn [user-id widget-id timezone size]
           (let [definition (get (handlers) widget-id)
                 size (or size (:default-size definition))]
             (cond
               (nil? definition) (widget-not-found! widget-id)
               (not (contains? (set (:supported-sizes definition)) size))
               (js/Promise.reject (ex-info "Choose a supported widget size." {:type :unsupported-size}))
               :else (js/Promise.resolve ((:fetch definition) user-id timezone size)))))
         :widgetSettings
         (fn [user-id widget-id]
           (let [definition (get (handlers) widget-id)
                 settings-get (:settings-get definition)]
             (cond
               (nil? definition) (widget-not-found! widget-id)
               (not (fn? settings-get))
               (js/Promise.reject
                (ex-info "This widget has no settings."
                         {:type :not-found :id widget-id}))
               :else (js/Promise.resolve (settings-get user-id)))))
         :saveWidgetSettings
         (fn [user-id widget-id values expected]
           (let [definition (get (handlers) widget-id)
                 settings-save (:settings-save definition)]
             (cond
               (nil? definition) (widget-not-found! widget-id)
               (not (fn? settings-save))
               (js/Promise.reject
                (ex-info "This widget has no settings."
                         {:type :not-found :id widget-id}))
               :else (js/Promise.resolve (settings-save user-id values expected)))))}))

(defn- require-session! [^js request response]
  (if-let [user (.-user request)]
    user
    (do (http/error! response 401 "unauthenticated"
                     "Authentication is required." {} (.-requestId response))
        nil)))

(defn- send-service-error! [^js response ^js error]
  (let [request-id (.-requestId response)
        data (ex-data error)
        [status code] (or (get {:precondition-required [428 "preconditionRequired"]
                               :precondition-failed [412 "preconditionFailed"]
                               :preference-conflict [412 "preconditionFailed"]} (:type data))
                          (get error-mapping (:type data))
                          (get error-mapping (:error data)))]
    (if (some? status)
      (http/error! response status code (ex-message error) {} request-id)
      (http/error! response 500 "internal" "The dashboard failed."
                   {} request-id))))

(defn- parse-layout-body! [value]
  (let [explanation (m/explain layout-schema value)]
    (when (some? explanation)
      (throw (ex-info "The dashboard layout is invalid."
                      {:type :invalid-layout :error :invalid-body})))))

(defn- query-timezone! [^js request]
  (let [timezone (or (str/trim (or (.get (.-query request) "timezone") ""))
                     "UTC")
        timezone (if (str/blank? timezone) "UTC" timezone)]
    (when-not (valid-timezone? timezone)
      (throw (ex-info "The timezone is invalid." {:type :invalid-timezone})))
    timezone))

(defn- register-api! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js store (service (.-preferences ctx) (.-contributions ctx))]
    (.get server "/api/v1/dashboard/catalog"
          (fn [^js request response _next]
            (when (require-session! request response)
              (http/json! response (.catalog store)))))
    (.get server "/api/v1/dashboard/layout"
          (fn [^js request response _next]
            (when (require-session! request response)
              (-> (.loadLayout store (.-id (.-user request)))
                  (.then (fn [layout]
                           (.set (.-headers response) "etag" (revisions/etag (:revision layout)))
                           (http/json! response layout)))
                  (.catch (fn [error] (send-service-error! response error)))))))
    (.put server "/api/v1/dashboard/layout"
          (fn [^js request response _next]
            (when (require-session! request response)
              (-> (http/read-json! request)
                  (.then (fn [body]
                           (let [value (js->clj body :keywordize-keys true)]
                             (parse-layout-body! value)
                             (.saveLayout store (.-id (.-user request)) value
                                          (revisions/required (.get (.-headers request) "if-match"))))))
                  (.then (fn [layout]
                           (.set (.-headers response) "etag" (revisions/etag (:revision layout)))
                           (http/json! response layout)))
                  (.catch (fn [error] (send-service-error! response error)))))))
    (.get server "/api/v1/dashboard/widgets/:id"
          (fn [^js request response _next]
            (when (require-session! request response)
              (-> (js/Promise.resolve nil)
                  (.then #(query-timezone! request))
                  (.then (fn [timezone]
                           (.widgetData store
                                        (.-id (.-user request))
                                        (.-id (.-params request))
                                        timezone
                                        (.get (.-query request) "size"))))
                  (.then (fn [data] (http/json! response data)))
                  (.catch (fn [error] (send-service-error! response error)))))))
    (.get server "/api/v1/dashboard/widgets/:id/settings"
          (fn [^js request response _next]
            (when (require-session! request response)
              (-> (js/Promise.resolve nil)
                  (.then #(query-timezone! request))
                  (.then (fn [timezone]
                           (.widgetSettings store
                                            (.-id (.-user request))
                                            (.-id (.-params request))
                                            timezone)))
                  (.then (fn [settings]
                           (when (some? (:revision settings)) (.set (.-headers response) "etag" (revisions/etag (:revision settings))))
                           (http/json! response settings)))
                  (.catch (fn [error] (send-service-error! response error)))))))
    (.put server "/api/v1/dashboard/widgets/:id/settings"
          (fn [^js request response _next]
            (when (require-session! request response)
              (-> (js/Promise.resolve nil)
                  (.then (fn [_] (revisions/required (.get (.-headers request) "if-match")) (http/read-json! request)))
                  (.then (fn [body]
                           (let [values (js->clj (.-values body))]
                             (when-not (and (map? values)
                                            (every? string? (keys values))
                                            (every? string? (vals values)))
                               (throw (ex-info "The widget settings are invalid."
                                               {:type :invalid-settings})))
                             (.saveWidgetSettings store
                                                  (.-id (.-user request))
                                                  (.-id (.-params request))
                                                  values (revisions/required (.get (.-headers request) "if-match"))))))
                  (.then (fn [settings]
                           (when (some? (:revision settings)) (.set (.-headers response) "etag" (revisions/etag (:revision settings))))
                           (http/json! response settings)))
                  (.catch (fn [error] (send-service-error! response error)))))))
    nil))

(def dashboard-plugin
  (cordis/plugin
   {:name "dar-dashboard"
    :inject ["contributions" "preferences" "server"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "dashboard" (service (.-preferences ctx)
                                                (.-contributions ctx)))
      (register-api! ctx _config))}))
