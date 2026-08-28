(ns dar.client.share-visitor
  (:require [clojure.string :as str]
            [reagent.core :as r]))

;; The public share visitor page (Stage 7): /share/{token} renders share
;; contents for people who are not signed in. It lives outside the
;; authenticated app shell and talks only to the public JSON namespace:
;; GET /share/{token}, POST /share/{token}/unlock,
;; GET /share/{token}/download?name=, and POST /share/{token}/upload?name=
;; for drop shares. Expiry, password gating, and rate limits stay
;; server-side; this page only reflects what the API allows.

(defn- share-json
  "Public API helper returning the parsed body, or rejecting with the
  error envelope's {:code :message}. Same-origin cookies ride along so an
  unlocked password share stays readable."
  [method path {:keys [body content-type]}]
  (let [headers #js {}]
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
                                  value
                                  (throw (ex-info "Share API error"
                                                  {:code (get-in value [:error :code])
                                                   :message (get-in value [:error :message])}))))))))))))

(defn- human-size [bytes]
  (cond
    (< bytes 1024) (str bytes " B")
    (< bytes (* 1024 1024)) (str (js/Math.round (/ bytes 102.4)) " KB")
    (< bytes (* 1024 1024 1024)) (str (/ (js/Math.round (/ bytes 10485.76)) 100) " MB")
    :else (str (/ (js/Math.round (/ bytes 10737418.24)) 100) " GB")))

(defn- friendly-error [code]
  (case code
    "notFound" "This link is no longer shared."
    "gone" "This link is no longer shared."
    "forbidden" "This link needs its password."
    "unauthenticated" "This link needs its password."
    "rateLimited" "Too many attempts. Wait a moment and try again."
    "Something went wrong with this link."))

(defn- download-href [token name]
  (str "/share/" (js/encodeURIComponent token)
       "/download?name=" (js/encodeURIComponent name)))

(defonce share-page-state
  (r/atom {:status :loading :info nil :password "" :error nil
           :flash nil :busy false :token nil}))

(defn- load! [token]
  (swap! share-page-state assoc :status :loading :token token :error nil)
  (-> (share-json "GET" (str "/share/" (js/encodeURIComponent token)) nil)
      (.then (fn [info]
               (swap! share-page-state assoc :status :ready :info info)))
      (.catch (fn [error]
                (let [code (:code (ex-data error))]
                  (if (or (= code "unauthenticated") (= code "forbidden"))
                    (swap! share-page-state assoc :status :password :error nil)
                    (swap! share-page-state assoc :status :error
                           :message (friendly-error code))))))))


(defn- password-screen [token state]
  (let [password (r/cursor state [:password])
        error (:error @state)]
    [:main.sanctuary-auth
     [:section.sanctuary-auth-card
      [:span.sanctuary-empty-symbol "D"]
      [:span.sanctuary-eyebrow "Shared with you"]
      [:h1 "This link is protected"]
      [:p "Enter the password the sender gave you to open it."]
      (when error
        [:p.sanctuary-auth-error {:role "alert"} error])
      [:form.sanctuary-share-unlock
       {:on-submit (fn [event]
                     (.preventDefault event)
                     (swap! state assoc :busy true :error nil)
                     (-> (share-json "POST" (str "/share/" (js/encodeURIComponent token) "/unlock")
                                     {:body (js/JSON.stringify #js {:password @password})
                                      :content-type "application/json"})
                     (.then (fn [_]
                              (swap! state assoc :busy false)
                              (load! token)))
                         (.catch (fn [error]
                                   (swap! state assoc :busy false
                                          :error (friendly-error (:code (ex-data error))))))))}
       [:label.sanctuary-share-label
        [:span "Password"]
        [:input {:type "password"
                 :name "password"
                 :required true
                 :value @password
                 :on-change #(swap! state assoc :password (.. % -target -value))}]]
       [:button.sanctuary-login-action
        {:type "submit" :disabled (:busy @state)}
        (if (:busy @state) "Checking…" "Open shared link")]]]]))

(defn- error-screen [message]
  [:main.sanctuary-auth
   [:section.sanctuary-auth-card {:aria-live "polite"}
    [:span.sanctuary-empty-symbol "D"]
    [:span.sanctuary-eyebrow "Shared with you"]
    [:h1 "This link is unavailable"]
    [:p message]]])

(defn- loading-screen []
  [:main.sanctuary-auth
   [:section.sanctuary-auth-card {:aria-live "polite"}
    [:span.sanctuary-empty-symbol "D"]
    [:h1 "Opening shared link"]
    [:div.sanctuary-auth-skeleton]
    [:p "Fetching what was shared with you…"]]])

(defn- drop-screen [token name state]
  (let [flash (:flash @state)
        busy (:busy @state)]
    [:section.sanctuary-panel.sanctuary-share-card
     [:div.sanctuary-panel-heading
      [:h2 name]
      [:span.sanctuary-eyebrow "Drop link"]]
     [:p "Send files to the owner of this link. They stay private to you both."]
     [:form.sanctuary-share-upload
      {:on-submit (fn [event]
                    (.preventDefault event)
                    (let [input (.getElementById js/document "share-drop-input")
                          file (some-> (.-files input) (aget 0))]
                      (if (nil? file)
                        (swap! state assoc :flash "Choose a file first.")
                        (do (swap! state assoc :busy true :flash nil)
                            (-> (.arrayBuffer file)
                                (.then (fn [buffer]
                                         (share-json "POST"
                                                     (str "/share/"
                                                          (js/encodeURIComponent token)
                                                          "/upload?name="
                                                          (js/encodeURIComponent (.-name file)))
                                                     {:body buffer
                                                      :content-type "application/octet-stream"})))
                                (.then (fn [_]
                                         (swap! state assoc :busy false
                                                :flash (str "Uploaded " (.-name file) "."))))
                                (.catch (fn [error]
                                          (swap! state assoc :busy false
                                                 :flash (friendly-error (:code (ex-data error)))))))))))}
      [:label.sanctuary-share-label
       [:span "File"]
       [:input {:id "share-drop-input"
                :type "file"
                :name "file"
                :required true}]]
      [:button.sanctuary-login-action
       {:type "submit" :disabled busy}
       (if busy "Uploading…" "Send file")]
      (when flash
        [:p.sanctuary-share-flash {:role "status"} flash])]]))

(defn- folder-screen [token name items]
  [:section.sanctuary-panel.sanctuary-share-card
   [:div.sanctuary-panel-heading
    [:h2 name]
    [:span.sanctuary-eyebrow "Shared folder"]]
   (if (empty? items)
     [:p.sanctuary-files-empty "Nothing is in this folder."]
     [:ul.sanctuary-list
      (for [entry items]
        ^{:key (:name entry)}
        [:li.sanctuary-share-row
         [:span.sanctuary-share-main
          [:strong (:name entry)]
          [:small (if (:isDir entry) "Folder" (human-size (:size entry)))]]
         (when-not (:isDir entry)
           [:span.sanctuary-share-commands
            [:a.sanctuary-files-open
             {:href (download-href token (:name entry))}
             "Download"]])])])])

(defn- file-screen [token name size]
  [:section.sanctuary-panel.sanctuary-share-card
   [:div.sanctuary-panel-heading
    [:h2 name]
    [:span.sanctuary-eyebrow "Shared file"]]
   [:p.sanctuary-share-meta (str "Size · " (human-size size))]
   [:a.sanctuary-login-action {:href (download-href token name)}
    "Download"]])

(defn page [token]
  ;; Loads the share once per token; re-renders with the same token keep
  ;; the loaded state.
  (r/with-let [_ (when (not= token (:token @share-page-state))
                   (js/setTimeout #(load! token) 0))]
    (let [{:keys [status info]} @share-page-state]
      (case status
        :loading [loading-screen]
        :password [password-screen token share-page-state]
        :error [error-screen (:message @share-page-state)]
        :ready (let [name (or (:name info) "Shared")]
                 (cond
                   (:file info) [file-screen token name (:size info)]
                   (:drop info) [drop-screen token name share-page-state]
                   :else [folder-screen token name (or (:items info) [])]))))))
