(ns dar.client.files
  (:require [dar.client.ui :as ui]
            [re-frame.core :as frame]
            [reagent.core :as r]
            ["dar-files/workspace" :refer [FilesPage]]))

;; Shared layout and focus primitives remain owned by the SPA shell. Files'
;; view, state, effects, editor, and persistence are TypeScript React components.
(def ^:private dialog-boundary
  (fn [^js props]
    (r/as-element [ui/dialog (.-onClose props) (.-children props)])))

(def ^:private sidebar-toggle
  (fn [] (r/as-element [ui/sidebar-toggle :files])))

(def ^:private detail-resizer
  (fn [] (r/as-element [ui/detail-resizer :files])))

(defn page []
  (let [session @(frame/subscribe [:session])]
    [:> FilesPage
     {:user (clj->js (:user session))
      :layoutAttrs (clj->js (ui/layout-attrs :files))
      :DialogBoundary dialog-boundary
      :SidebarToggle sidebar-toggle
      :DetailResizer detail-resizer
      :formatTime (fn [value]
                    (ui/format-date (js/Date. value)
                                    {:year "numeric" :month "numeric" :day "numeric"
                                     :hour "numeric" :minute "2-digit"}))}]))
