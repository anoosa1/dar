(ns dar.client.files
  (:require [clojure.string :as str]
            [dar.client.state :as state]
            [re-frame.core :as frame]
            [reagent.core :as r]))

;; The Files page (Stage 1): browse folders, create folders and empty
;; files, upload files, toggle favorites, and send entries to trash.
;; Folder state lives in the app db; trash browsing, versions, and the
;; text editor follow in later stages.

(defn- csrf-token []
  (state/cookie-value "dar-csrf"))

(defn- human-size [bytes]
  (cond
    (< bytes 1024) (str bytes " B")
    (< bytes (* 1024 1024)) (str (js/Math.round (/ bytes 102.4)) " KB")
    (< bytes (* 1024 1024 1024)) (str (/ (js/Math.round (/ bytes 10485.76)) 100) " MB")
    :else (str (/ (js/Math.round (/ bytes 10737418.24)) 100) " GB")))

(defn- friendly-error [code]
  (case code
    "conflict" "Something with that name already exists."
    "notFound" "That entry is no longer there."
    "tooLarge" "That upload is too large for this instance."
    "quotaExceeded" "Your storage quota is exhausted."
    "invalid" "That path is not allowed."
    "unauthenticated" "Your session ended. Sign in again."
    "Something went wrong."))

(defn- files-json
  "Files API helper returning {:ok :status :body} or rejecting with the
  error envelope's {:code :message}. Session cookies and the CSRF token
  are attached to every mutation."
  [method path {:keys [body content-type]}]
  (let [headers #js {"x-csrf-token" (or (csrf-token) "")}]
    (when content-type
      (aset headers "content-type" content-type))
    (-> (js/fetch path #js {:method method
                            :credentials "same-origin"
                            :headers headers
                            :body body})
        (.then (fn [response]
                 (-> (.json response)
                     (.then (fn [parsed]
                              (let [value (js->clj parsed :keywordize-keys true)]
                                (if (.-ok response)
                                  {:ok true :status (.-status response) :body value}
                                  (throw (ex-info "Files API error"
                                                  {:code (get-in value [:error :code])
                                                   :message (get-in value [:error :message])}))))))))))))

(defn- fetch-folder! [path]
  (-> (files-json "GET" (str "/api/v1/files?path=" (js/encodeURIComponent path)) nil)
      (.then (fn [{:keys [body]}]
               (frame/dispatch [:files/loaded path (:items body)])))
      (.catch (fn [error]
                (frame/dispatch [:files/failed path
                                 (friendly-error (:code (ex-data error)))])))))

(defn- mutate!
  "Runs one mutation and reloads the folder either way it settles, so a
  rejected mutation snaps the client back to server truth."
  [current-path mutation]
  (-> (mutation)
      (.catch (fn [error]
                (frame/dispatch [:files/flash (friendly-error (:code (ex-data error)))])))
      (.then (fn [_] (fetch-folder! current-path)))))

(frame/reg-fx
 :files/fetch
 (fn [[path]] (fetch-folder! path)))

(frame/reg-fx
 :files/action
 (fn [[current-path mutation]]
   (mutate! current-path mutation)))

;; --- state ---

(def default-files
  {:path "/"
   :status :loading
   :items []
   :error nil
   :flash nil
   :creating nil
   :confirm-delete nil})

(frame/reg-sub
 :files
 (fn [db _]
   (merge default-files (:files db))))

(frame/reg-event-fx
 :files/open
 (fn [{:keys [db]} [_ path]]
   {:db (assoc db :files (assoc default-files :path path))
    :files/fetch [path]}))

(frame/reg-event-db
 :files/loaded
 (fn [db [_ path items]]
   (if (= path (get-in db [:files :path]))
     (-> db
         (assoc-in [:files :status] :ready)
         (assoc-in [:files :items] (vec items))
         (assoc-in [:files :error] nil))
     db)))

(frame/reg-event-db
 :files/failed
 (fn [db [_ path message]]
   (if (= path (get-in db [:files :path]))
     (-> db
         (assoc-in [:files :status] :error)
         (assoc-in [:files :error] message))
     db)))

(frame/reg-event-db
 :files/flash
 (fn [db [_ message]]
   (-> db
       (assoc-in [:files :flash] message)
       (assoc-in [:files :confirm-delete] nil))))

(frame/reg-event-db
 :files/dismiss-flash
 (fn [db _]
   (assoc-in db [:files :flash] nil)))

(frame/reg-event-db
 :files/start-creating
 (fn [db [_ kind]]
   (-> db
       (assoc-in [:files :creating] kind)
       (assoc-in [:files :flash] nil))))

(frame/reg-event-db
 :files/cancel-creating
 (fn [db _]
   (assoc-in db [:files :creating] nil)))

(frame/reg-event-fx
 :files/create
 (fn [{:keys [db]} [_ kind name]]
   (let [current-path (get-in db [:files :path] "/")
         target (str (if (= current-path "/") "" current-path) "/" name)
         route (if (= kind :folder)
                 "/api/v1/files/folders"
                 "/api/v1/files/files")]
     {:db (assoc-in db [:files :creating] nil)
      :files/action
      [current-path
       (fn []
         (files-json "POST" route
                     {:body (js/JSON.stringify (clj->js {:path target}))
                      :content-type "application/json"}))]})))

(frame/reg-event-fx
 :files/delete
 (fn [{:keys [db]} [_ entry-path]]
   (let [current-path (get-in db [:files :path] "/")]
     {:db (assoc-in db [:files :confirm-delete] nil)
      :files/action
      [current-path
       (fn []
         (files-json "DELETE"
                     (str "/api/v1/files/files?path="
                          (js/encodeURIComponent entry-path))
                     nil))]})))

(frame/reg-event-db
 :files/toggle-confirm-delete
 (fn [db [_ entry-path]]
   (if (= entry-path (get-in db [:files :confirm-delete]))
     (assoc-in db [:files :confirm-delete] nil)
     (assoc-in db [:files :confirm-delete] entry-path))))

(frame/reg-event-fx
 :files/toggle-favorite
 (fn [{:keys [db]} [_ entry-path favorited]]
   (let [current-path (get-in db [:files :path] "/")]
     {:files/action
      [current-path
       (fn []
         (files-json "POST" "/api/v1/files/favorite"
                     {:body (js/JSON.stringify
                             (clj->js {:path entry-path :favorited favorited}))
                      :content-type "application/json"}))]})))

(frame/reg-fx
 :files/upload
 (fn [[current-path ^js file]]
   (let [target (str (if (= current-path "/") "" current-path) "/" (.-name file))]
     (-> (.arrayBuffer file)
         (.then (fn [buffer]
                  (files-json "POST"
                              (str "/api/v1/files/upload?path="
                                   (js/encodeURIComponent target))
                              {:body buffer
                               :content-type (or (.-type file)
                                                 "application/octet-stream")})))
         (.catch (fn [error]
                   (frame/dispatch [:files/flash
                                    (friendly-error (:code (ex-data error)))])))
         (.then (fn [_]
                  (frame/dispatch [:files/open current-path])))))))

(frame/reg-event-fx
 :files/upload-selected
 (fn [{:keys [db]} [_ ^js file]]
   (when file
     {:db (assoc-in db [:files :confirm-delete] nil)
      :files/upload [(get-in db [:files :path] "/") file]})))

;; --- view ---

(defn- parent-path [path]
  (if-let [index (str/last-index-of path "/")]
    (if (zero? index) "/" (subs path 0 index))
    "/"))

(defn- crumbs
  "Builds [{:label :path current?}] from a virtual path."
  [current]
  (let [segments (remove str/blank? (str/split current #"/"))]
    (into [{:label "Home" :path "/" :current? (= current "/")}]
          (rest
           (reductions (fn [{:keys [path]} segment]
                         {:label segment
                          :path (str (if (= path "/") "" path) "/" segment)
                          :current? false})
                       {:path "/"}
                       segments)))))

(defn- breadcrumb [current]
  [:nav.sanctuary-crumbs {:aria-label "Folder path"}
   (for [{:keys [label path current?]} (crumbs current)]
     (if current?
       [:span {:key path :aria-current "location"} label]
       [:a {:key path
            :href "/files"
            :on-click #(frame/dispatch [:files/open path])}
        label]))])

(defn- create-form [kind]
  (let [local (r/atom "")]
    (fn [kind]
      [:form.sanctuary-files-create
       {:on-submit (fn [event]
                     (.preventDefault event)
                     (let [value (str/trim @local)]
                       (when-not (str/blank? value)
                         (frame/dispatch [:files/create kind value]))))}
       [:input {:type "text"
                :value @local
                :required true
                :auto-focus true
                :placeholder (if (= kind :folder)
                               "New folder name"
                               "New file name")
                :aria-label (if (= kind :folder)
                              "New folder name"
                              "New file name")
                :on-change #(reset! local (.. % -target -value))}]
       [:button {:type "submit"} "Create"]
       [:button {:type "button"
                 :on-click #(frame/dispatch [:files/cancel-creating])}
        "Cancel"]])))

(defn- entry-row [{:keys [path name isDir size favorited mimeType updatedAt]}
                  confirm-delete?]
  [:li.sanctuary-files-row
   [:span.sanctuary-files-main
    (if isDir
      [:button.sanctuary-files-open
       {:type "button"
        :on-click #(frame/dispatch [:files/open path])}
       name]
      [:span name])
    [:small.sanctuary-files-meta
     (if isDir
       "Folder"
       (str (human-size size) " · " mimeType
            (when updatedAt
              (str " · " (.toLocaleString (js/Date. updatedAt))))))]]
   [:span.sanctuary-files-commands
    [:button.sanctuary-files-star
     {:type "button"
      :aria-pressed favorited
      :aria-label (if favorited
                    (str "Remove " name " from favorites")
                    (str "Add " name " to favorites"))
      :on-click #(frame/dispatch [:files/toggle-favorite path (not favorited)])}
     (if favorited "★" "☆")]
    (if confirm-delete?
      [:span.sanctuary-files-confirm
       [:button {:type "button"
                 :on-click #(frame/dispatch [:files/delete path])}
        "Confirm delete"]
       [:button {:type "button"
                 :on-click #(frame/dispatch [:files/toggle-confirm-delete path])}
        "Keep"]]
      [:button {:type "button"
                :aria-label (str "Send " name " to trash")
                :on-click #(frame/dispatch [:files/toggle-confirm-delete path])}
       "Delete"])]])

(defn- files-panel []
  (let [{:keys [path status items error flash creating confirm-delete]}
        @(frame/subscribe [:files])]
    (cond
      (= status :loading)
      [:div.sanctuary-auth-skeleton {:aria-hidden "true"}]

      (= status :error)
      [:p.sanctuary-auth-error {:role "alert"} (or error "Files are unavailable.")]

      :else
      [:div.sanctuary-files
       [breadcrumb path]
       (when flash
         [:p.sanctuary-auth-error {:role "status"} flash " "
          [:button {:type "button"
                    :on-click #(frame/dispatch [:files/dismiss-flash])}
           "Dismiss"]])
       [:div.sanctuary-files-toolbar
        [:button {:type "button"
                  :on-click #(frame/dispatch [:files/start-creating :folder])}
         "New folder"]
        [:button {:type "button"
                  :on-click #(frame/dispatch [:files/start-creating :file])}
         "New file"]
        [:label.sanctuary-files-upload
         "Upload file"
         [:input {:type "file"
                  :on-change (fn [event]
                               (frame/dispatch [:files/upload-selected
                                                (.. event -target -files (item 0))])
                               (set! (.. event -target -value) ""))}]]
        (when-not (= path "/")
          [:button {:type "button"
                    :on-click #(frame/dispatch [:files/open (parent-path path)])}
           "Up one level"])]
       (case creating
         :folder [create-form :folder]
         :file [create-form :file]
         nil)
       (if (empty? items)
         [:p.sanctuary-files-empty "This folder is empty."]
         [:ul.sanctuary-list
          (for [item items]
            ^{:key (:path item)}
            [entry-row item (= (:path item) confirm-delete)])])])))

(defn page []
  (r/create-class
   {:component-did-mount
    (fn []
      ;; The Files panel owns its folder state, so every visit reloads
      ;; from the server rather than trusting stale client state.
      (frame/dispatch [:files/open "/"]))
    :reagent-render
    (fn []
      [:section.sanctuary-panel
       [:div.sanctuary-panel-heading [:h2 "Your files"]]
       [files-panel]])}))
