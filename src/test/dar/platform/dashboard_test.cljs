(ns dar.platform.dashboard-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.dashboard :as dashboard]
            [dar.platform.database :as database]
            [dar.platform.preferences :as preferences]
            [dar.platform.contributions :as contributions]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(defn- fixture-definitions []
  {"clock.widget" {:id "clock.widget" :title "Clock" :kind "clock"
                   :defaultSize "standard"
                   :supportedSizes ["compact" "standard" "wide" "large"]}
   "notes.widget" {:id "notes.widget" :title "Notes" :kind "notes"
                   :defaultSize "standard"
                   :supportedSizes ["standard" "wide"]}})

(defn- fixture-widgets []
  (vals (fixture-definitions)))

(deftest default-layout-packs-across-twelve-columns
  (let [layout (dashboard/default-layout (fixture-widgets))
        items (:layout layout)]
    (is (= 2 (count items)))
    (is (= 0 (:x (first items))))
    (is (= 4 (:x (second items))))
    (is (every? #(= "standard" (:size %)) items))))

(deftest default-layout-wraps-when-the-row-is-full
  (let [widgets (vec (repeat 4 {:id "clock.widget" :title "Clock" :kind "clock"
                                :defaultSize "standard"
                                :supportedSizes ["standard"]}))
        items (:layout (dashboard/default-layout widgets))]
    (is (= 4 (count items)))
    (is (= 0 (:y (first items))))
    (is (= 3 (:y (last items))))))

(deftest normalize-drops-unknown-and-duplicates
  (let [[normalized changed] (dashboard/normalize-layout
                              {:layout [{:id "clock.widget" :x 0 :y 0 :size "standard"}
                                        {:id "clock.widget" :x 5 :y 5 :size "standard"}
                                        {:id "gone.widget" :x 0 :y 0 :size "standard"}]}
                              (fixture-definitions))]
    (is (true? changed))
    (is (= ["clock.widget"] (map :id (:layout normalized))))))

(deftest saved-layout-normalization-repairs-malformed-generic-preferences
  (let [definitions (fixture-definitions)
        [normalized changed] (dashboard/normalize-layout
                              {:layout [nil "broken" {:id "clock.widget" :x "not-a-number" :y "not-a-number" :size "wide"}
                                        {:id "notes.widget" :x -50 :y 9 :size "unsupported"}]} definitions)]
    (is changed)
    (is (= 2 (count (:layout normalized))))
    (is (every? #(and (js/Number.isSafeInteger (:x %)) (<= 0 (:x %) 11)
                      (js/Number.isSafeInteger (:y %)) (<= 0 (:y %) dashboard/max-y)) (:layout normalized)))
    (is (= #{"wide" "standard"} (set (map :size (:layout normalized)))))
    (doseq [value ["broken" {:layout "broken"} {:layout nil}]]
      (let [[repaired changed] (dashboard/normalize-layout value definitions)]
        (is changed) (is (= 2 (count (:layout repaired)))))))
  (let [[repaired changed] (dashboard/normalize-layout {:layout [{:id "clock.widget" :x 1000000000 :y 1000000000 :size "standard"}]}
                                                      (fixture-definitions))]
    (is changed)
    (is (= [{:id "clock.widget" :x 8 :y 0 :size "standard"}] (:layout repaired)))))

(deftest validate-rejects-broken-layouts
  (let [definitions (fixture-definitions)]
    (is (= :unknown-widget
           (dashboard/validate-layout
            {:layout [{:id "nope.widget" :x 0 :y 0 :size "standard"}]} definitions)))
    (is (= :duplicate-widget
           (dashboard/validate-layout
            {:layout [{:id "clock.widget" :x 0 :y 0 :size "standard"}
                      {:id "clock.widget" :x 4 :y 0 :size "standard"}]} definitions)))
    (is (= :unsupported-size
           (dashboard/validate-layout
            {:layout [{:id "notes.widget" :x 0 :y 0 :size "compact"}]} definitions)))
    (is (= :invalid-coordinates
           (dashboard/validate-layout
            {:layout [{:id "clock.widget" :x 12 :y 0 :size "standard"}]} definitions)))
    (is (= :invalid-coordinates
           (dashboard/validate-layout
            {:layout [{:id "clock.widget" :x 0 :y 10001 :size "standard"}]} definitions)))
    (is (= :active-widget-missing
           (dashboard/validate-layout
            {:layout [{:id "clock.widget" :x 0 :y 0 :size "standard"}]
             :activeId "notes.widget"} definitions)))
    (is (nil? (dashboard/validate-layout
               {:layout [{:id "clock.widget" :x 0 :y 0 :size "standard"}]} definitions)))))

(deftest reflow-resolves-collisions-by-pushing-down
  (let [definitions (fixture-definitions)
        layout (dashboard/reflow
                {:layout [{:id "clock.widget" :x 0 :y 0 :size "wide"}
                          {:id "notes.widget" :x 0 :y 0 :size "standard"}]}
                definitions)]
    (is (= 0 (:y (first (:layout layout)))))
    (is (> (:y (second (:layout layout))) 0))))

(deftest reflow-compacts-freed-space
  (let [definitions (fixture-definitions)
        layout (dashboard/reflow
                {:layout [{:id "clock.widget" :x 0 :y 0 :size "standard"}
                          {:id "notes.widget" :x 4 :y 0 :size "standard"}
                          {:id "notes.widget" :x 0 :y 3 :size "standard"}]}
                definitions)]
    ;; "notes.widget" appears twice before normalization; reflow keeps both
    ;; because duplicates are a validation concern, not a geometry one.
    (is (= 3 (count (:layout layout))))
    (is (= 0 (:y (first (sort-by (fn [i] [(:y i) (:x i)]) (:layout layout))))))))

(deftest compaction-is-stable-after-a-single-save
  (let [widgets (mapv #(hash-map :id (str "widget-" %) :defaultSize "standard"
                                 :supportedSizes ["standard" "wide" "large"]) (range 7))
        definitions (into {} (map (juxt :id identity) widgets))
        initial {:layout (mapv (fn [index widget]
                                {:id (:id widget) :x 0 :y (* index 10)
                                 :size (if (zero? index) "wide" "standard")}) (range) widgets)
                 :activeId ""}
        saved (dashboard/reflow initial definitions)]
    (is (= saved (dashboard/reflow saved definitions)))
    (is (= [saved false] (dashboard/normalize-layout saved definitions)))
    (is (= [0 4 7 10 13 16 19] (mapv :y (:layout saved))))))

(deftest catalog-lists-registered-widget-definitions
  (let [^js registry (contributions/registry)]
    (dashboard/register-widget!
     registry "files.favorites"
     {:id "files.favorites" :title "Favorites" :kind "files"
      :default-size "standard" :supported-sizes ["compact" "standard"]
      :fetch (fn [_user _timezone _size] (js/Promise.resolve {}))})
    (is (thrown-with-msg? js/Error #"supported sizes"
                          (dashboard/register-widget!
                           registry "test.broken"
                           {:id "test.broken" :title "Broken" :kind "files"
                            :default-size "giant" :supported-sizes ["standard"]
                            :fetch (fn [])})))
    (let [widgets (dashboard/catalog registry)
          entry (first (filter (fn [w] (= "files.favorites" (:id w))) widgets))]
      (is (= 1 (count widgets)))
      (is (= "files.favorites" (:id entry)))
      (is (= "standard" (:defaultSize entry))))))

(defn- open-stores! []
  (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-dashboard-test-"))
        connection (database/open! (.resolve path directory "dar.sqlite"))
        ^js preferences (preferences/service #js {:connection connection
                                                  :writer (database/writer-queue connection)})
        ^js registry (contributions/registry)]
    (dashboard/register-widget!
     registry "clock.widget"
     {:id "clock.widget" :title "Clock" :kind "clock"
      :default-size "standard" :supported-sizes ["standard"]
      :configurable true
      :fetch (fn [_user _timezone _size]
               (js/Promise.resolve {:sections []}))
      :settings-get (fn [_user]
                      (js/Promise.resolve {:values {"format" "24h"}}))
      :settings-save (fn [_user values]
                       (js/Promise.resolve {:values values}))})
    {:directory directory
     :connection connection
     :registry registry
     :preferences preferences
     :store (dashboard/service preferences registry)}))

(defn- seed-user! [fixture]
  (let [^js connection (:connection fixture)]
    (.run (.prepare connection
                    "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?)")
          "user-1" "dash-subject" "Dash" "dash@example.test"
          (.toISOString (js/Date.)) (.toISOString (js/Date.)))))
(defn- close-stores! [fixture]
  (.close (:connection fixture))
  (.rmSync fs (:directory fixture) #js {:recursive true :force true}))

(defn- settle
  "Maps a promise to a resolved {:outcome ...} step that never rejects."
  [promise outcome]
  (-> promise
      (.then (fn [value] {:outcome outcome :value value}))
      (.catch (fn [error] {:outcome outcome :error error}))))

(defn- run-steps
  "Reduces promise-returning thunks left to right; a rejecting thunk fails the
  whole run. Each thunk owns its own assertions."
  [thunks]
  (reduce (fn [prior thunk] (.then prior thunk))
          (js/Promise.resolve nil)
          thunks))

(defn- finish
  "Ends an async test exactly once, closing the stores first on either path."
  [start fixture done]
  (-> start
      (.then (fn [_]
               (close-stores! fixture)
               (done)))
      (.catch (fn [error]
                (is false (str "flow failed: " (.-message error)))
                (close-stores! fixture)
                (done)))))

(deftest layout-round-trips-through-preferences
  (async done
    (let [fixture (open-stores!)
          ^js store (:store fixture)
          save (.-saveLayout store)
          load (.-loadLayout store)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (load "user-1"))
         (fn [default-layout]
           (is (= 1 (count (:layout default-layout))))
           (is (= "clock.widget" (:id (first (:layout default-layout))))))
         (fn [_]
           (save "user-1"
                 {:layout [{:id "clock.widget" :x 8 :y 0 :size "standard"}]
                  :activeId ""}))
         (fn [saved]
           (is (= 8 (:x (first (:layout saved))))))
         (fn [_] (load "user-1"))
         (fn [reloaded]
           (is (= 8 (:x (first (:layout reloaded))))))])
       fixture done))))

(deftest save-rejects-invalid-layouts
  (async done
    (let [fixture (open-stores!)
          ^js store (:store fixture)
          save (.-saveLayout store)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (settle (save "user-1"
                         {:layout [{:id "gone.widget" :x 0 :y 0 :size "standard"}]})
                   :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :unknown-widget (:error (ex-data (:error step))))))
         (fn [_]
           (settle (save "user-1"
                         {:layout [{:id "clock.widget" :x 12 :y 0 :size "standard"}]})
                   :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :invalid-coordinates (:error (ex-data (:error step))))))])
       fixture done))))

(deftest widget-data-and-settings-flow
  (async done
    (let [fixture (open-stores!)
          ^js store (:store fixture)
          data (.-widgetData store)
          settings (.-widgetSettings store)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (data "user-1" "clock.widget" "UTC" "standard"))
         (fn [payload]
           (is (= {:sections []} payload)))
         (fn [_] (settings "user-1" "clock.widget" "UTC"))
         (fn [settings-payload]
           (is (= {"format" "24h"} (:values settings-payload))))
         (fn [_]
           (settle (data "user-1" "gone.widget" "UTC" "standard") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :not-found (:type (ex-data (:error step))))))])
       fixture done))))
