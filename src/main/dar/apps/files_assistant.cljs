(ns dar.apps.files-assistant
  (:require [dar.apps.files :as files]
            [dar.platform.assistant :as assistant]
            [dar.platform.dashboard :as dashboard]
            ["#dar-files/assistant" :as files-tools]
            ["node:path" :as path]))

;; The shared assistant registry still consumes Malli schemas and CLJS maps.
;; Tool definitions, effects, behavior, and widget payloads live in TypeScript.
(defn- schema [properties]
  (into [:map {:closed true}]
        (map (fn [^js property]
               [(keyword (.-name property)) {:optional (boolean (.-optional property))}
                (case (.-type property)
                  "string" :string
                  "boolean" :boolean
                  "positive" [:int {:min 1}]
                  "natural" [:int {:min 0}]
                  "enum" (into [:enum] (array-seq (.-values property))))]))
        (array-seq properties)))

(defn- result [promise]
  (-> promise (.then #(js->clj % :keywordize-keys true))
      (.catch (fn [error] (throw (files/translate-error error))))))

(defn list-source-dispatch! [^js store ^js user source share-id vpath]
  (result (files-tools/listSource (or (.-typescriptEngine store) store)
                                 (.-id user) source share-id vpath)))

(defn read-source-dispatch! [^js store ^js user source share-id vpath]
  (result (files-tools/readSource (or (.-typescriptEngine store) store)
                                 (.-id user) source share-id vpath)))

(defn register-tools! [^js registry ^js store ^js activity]
  (let [tools (files-tools/filesTools
               (.-typescriptEngine store)
               #js {:record
                    (fn [user event vpath recipients target-url]
                      (if activity
                        (let [deleted? (contains? #{"fileTrashed" "filePurged"} event)
                              url (or target-url (cond deleted? "/files?view=trash"
                                        (= event "folderCreated") (str "/files?path=" (js/encodeURIComponent vpath))
                                        :else (str "/files?path=" (js/encodeURIComponent (.dirname path vpath))
                                                   "&item=" (js/encodeURIComponent vpath))))]
                          (.record activity user "files" event
                                   {:path vpath :name (.basename path vpath) :url url :deleted deleted?}
                                   (vec (array-seq recipients))))
                        (js/Promise.resolve nil)))})]
    (doseq [^js tool (array-seq tools)]
      (assistant/register-tool!
        registry (.-name tool)
        {:name (.-name tool) :title (.-title tool) :effect (.-effect tool)
         :schema (schema (.-properties tool))
         :invoke (fn [^js user arguments]
                   (result (.invoke tool (.-id user) (clj->js arguments))))})))
  store)

(defn register-widget! [^js registry ^js store]
  (dashboard/register-widget!
    registry "files.favorites"
    {:id "files.favorites" :title "Favorite files" :kind "list"
     :default-size "standard" :supported-sizes ["compact" "standard" "wide" "large"]
     :fetch (fn [user _timezone size]
              (result (files-tools/favoritesWidget (.-typescriptEngine store) user size)))})
  store)
