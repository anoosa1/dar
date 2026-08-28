(ns dar.cordis
  (:require ["cordis" :refer [Context]]))

(defn context
  "Creates one isolated Cordis root context. The caller owns its lifecycle."
  []
  (new Context))

(defn plugin
  "Builds a Cordis plugin function with explicit injected service names."
  [{:keys [name inject apply]}]
  (let [entry (fn [ctx config]
                (apply ctx (js->clj config :keywordize-keys true)))]
    (js/Object.defineProperty entry "name"
                              #js {:value name
                                   :configurable true})
    (aset entry "inject" (clj->js (or inject [])))
    entry))

(defn mount!
  ([^js ctx plugin-entry]
   (js/Promise.resolve (.plugin ctx plugin-entry)))
  ([^js ctx plugin-entry config]
   (js/Promise.resolve (.plugin ctx plugin-entry (clj->js config)))))

(defn effect!
  "Registers an acquire function whose returned disposer belongs to ctx."
  [^js ctx label acquire]
  (.effect ctx acquire label))

(defn provide!
  "Publishes a service whose lifetime is owned by ctx."
  [^js ctx service-name value]
  (.provide ctx service-name value))

(defn dispose!
  "Disposes a context and every nested Cordis effect in reverse order."
  [^js ctx]
  (let [^js fiber (.-fiber ctx)]
    (.dispose fiber)))
