(ns dar.platform.instance
  (:require [dar.cordis :as cordis]
            [malli.core :as m]
            ["node:crypto" :refer [randomUUID]]
            ["node:fs/promises" :as fs]
            ["node:path" :as path]))

(def data-version 1)

(def instance-schema
  [:map {:closed true}
   [:product [:= "dar"]]
   [:dataVersion [:= data-version]]
   [:instanceId [:string {:min 1}]]
   [:database [:= "dar.sqlite"]]])

(defn validate-instance
  "Validates and keywordizes an instance marker without accepting future formats."
  [value]
  (let [instance (js->clj value :keywordize-keys true)]
    (when-not (m/validate instance-schema instance)
      (throw (ex-info "The data root belongs to an unsupported Dar version."
                      {:type :unsupported-data-root})))
    instance))

(defn- missing? [error]
  (= "ENOENT" (.-code error)))

(defn- read-instance! [marker-path]
  (-> (.readFile fs marker-path "utf8")
      (.then (fn [contents]
               (try
                 (validate-instance (js/JSON.parse contents))
                 (catch :default error
                   (if (= :unsupported-data-root (:type (ex-data error)))
                     (throw error)
                     (throw (ex-info "The instance marker is not valid JSON."
                                     {:type :unsupported-data-root}
                                     error)))))))))

(defn- sync-directory! [directory]
  (-> (.open fs directory "r")
      (.then (fn [^js handle]
               (-> (.sync handle)
                   (.finally #(.close handle)))))))

(defn- write-instance! [data-root marker-path]
  (let [instance {:product "dar"
                  :dataVersion data-version
                  :instanceId (randomUUID)
                  :database "dar.sqlite"}
        temporary (.resolve path data-root
                            (str ".instance." (:instanceId instance) ".tmp"))
        contents (str (js/JSON.stringify (clj->js instance) nil 2) "\n")]
    (-> (.open fs temporary "wx" 0600)
        (.then (fn [^js handle]
                 (-> (.writeFile handle contents "utf8")
                     (.then #(.sync handle))
                     (.finally #(.close handle)))))
        (.then #(.rename fs temporary marker-path))
        (.then #(sync-directory! data-root))
        (.then (fn [] instance))
        (.catch (fn [error]
                  (-> (.rm fs temporary #js {:force true})
                      (.catch (fn [_] nil))
                      (.then (fn [] (throw error)))))))))

(defn ensure!
  "Creates a fresh instance marker or validates an existing Dar v1 data root.

  A non-empty unmarked directory is rejected without modification."
  [data-root]
  (let [marker-path (.resolve path data-root "instance.json")]
    (-> (.mkdir fs data-root #js {:recursive true :mode 0700})
        (.then #(.lstat fs data-root))
        (.then (fn [^js stat]
                 (when (.isSymbolicLink stat)
                   (throw (ex-info "The Dar data root cannot be a symbolic link."
                                   {:type :unsafe-data-root})))
                 (.readdir fs data-root)))
        (.then (fn [entries]
                 (if (.includes entries "instance.json")
                   (read-instance! marker-path)
                   (if (zero? (.-length entries))
                     (write-instance! data-root marker-path)
                     (throw (ex-info
                             "Refusing to use a non-empty directory without a Dar v1 instance marker."
                             {:type :unrecognized-data-root
                              :entries (vec (js->clj entries))}))))))
        (.then (fn [instance]
                 (let [resolved (assoc instance
                                       :dataRoot data-root
                                       :databasePath (.resolve path data-root "dar.sqlite"))]
                   (-> (js/Promise.all
                        #js [(.mkdir fs (.resolve path data-root "users")
                                     #js {:recursive true :mode 0700})
                             (.mkdir fs (.resolve path data-root "plugins")
                                     #js {:recursive true :mode 0700})])
                       (.then (fn [] resolved)))))))))

(def instance-plugin
  (cordis/plugin
   {:name "dar-instance"
    :apply (fn [ctx {:keys [dataRoot]}]
             (.then (ensure! dataRoot)
                    (fn [instance]
                      (cordis/provide! ctx "instance" (clj->js instance))
                      nil)))}))
