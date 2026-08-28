(ns dar.platform.contributions
  (:require [dar.cordis :as cordis]))

(def contribution-kinds
  #{"routes" "navigation" "widgets" "searchProviders" "assistantTools"
    "preferences" "eventSubscriptions" "workers"})

(defn registry
  "Owns runtime contributions keyed by kind and namespaced id."
  []
  (let [by-kind (js/Map.)
        kind-map (fn [kind]
                   (when-not (.has by-kind kind)
                     (.set by-kind kind (js/Map.)))
                   (.get by-kind kind))]
    #js {:register
         (fn [kind id contribution]
           (when-not (contains? contribution-kinds kind)
             (throw (ex-info "Unknown contribution kind."
                             {:type :invalid-contribution :kind kind})))
           (when-not (and (string? id) (re-matches #"[a-z][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)+" id))
             (throw (ex-info "Contribution ids must be dotted and namespaced."
                             {:type :invalid-contribution :id id})))
           (let [contributions (kind-map kind)]
             (when (.has contributions id)
               (throw (ex-info "The contribution id is already registered."
                               {:type :duplicate-contribution :kind kind :id id})))
             (.set contributions id contribution)
             (fn []
               (if (= (.get contributions id) contribution)
                 (.delete contributions id)
                 nil))))
         :list
         (fn [kind]
           (when-not (contains? contribution-kinds kind)
             (throw (ex-info "Unknown contribution kind."
                             {:type :invalid-contribution :kind kind})))
           (if (.has by-kind kind)
             (.entries (.get by-kind kind))
             (js/Map.)))}))

(defn- register-routes! [^js ctx _config]
  (let [registry (registry)]
    (cordis/provide! ctx "contributions" registry)
    nil))

(def contributions-plugin
  (cordis/plugin
   {:name "dar-contributions"
    :apply register-routes!}))
