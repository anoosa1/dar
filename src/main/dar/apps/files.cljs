(ns dar.apps.files
  (:require [dar.platform.atomic-files :as atomic-files]
            [dar.platform.path :as path]
            [dar.platform.search :as search-index]
            ["#dar-files/engine" :as files-ts]
            ["#dar-files/content" :as content-ts]
            ["#dar-files/contracts" :refer [FilesError]]))

;; Files lives in TypeScript. This adapter is the data boundary for apps and
;; platform services that still use ClojureScript maps; it owns no Files rules.
(def editor-limit content-ts/EDITOR_LIMIT)
(defn mime-for-name [name] (content-ts/mimeForName name))

(defn translate-error [error]
  (if (instance? FilesError error)
    (ex-info (.-message error)
             (assoc (js->clj (.-details error) :keywordize-keys true)
                    :type (keyword (.-type error))))
    error))

(defn- from-js [value] (js->clj value :keywordize-keys true))

(defn- to-js [value]
  (clj->js
   (if (map? value)
     (reduce-kv (fn [result key item]
                  (assoc result (get {:if-match :ifMatch :expires-at :expiresAt
                                     :to-user :toUser :mime-prefixes :mimePrefixes} key key)
                         item)) {} value)
     value)))

(defn- wrap-engine [^js engine]
  (let [result #js {:typescriptEngine engine}]
    (doseq [name ["filesRoot" "quotaUsage" "list" "mkdir" "touch" "save"
                  "favorite" "delete" "listTrash" "deleteDocument"
                  "readTrashDocument" "listFavorites" "listTagged"
                  "reconcileSearch" "restore" "purge" "rename" "copy"
                  "readDocument" "saveDocument" "readDocumentSnapshot"
                  "createVersion" "listVersions" "readVersion" "restoreVersion"
                  "listTags" "createTag" "deleteTag" "setFileTag"
                  "listComments" "addComment" "deleteComment"
                  "lockInfo" "lock" "unlock" "search" "stats" "picker"
                  "pickerValidate" "diffVersions" "stat" "readFile" "preview"
                  "thumbnail" "createShare" "listShares" "deleteShare"
                  "readShare" "unlockShare" "isShareUnlocked" "uploadToShare"
                  "createUserShare" "deleteUserShare" "listUserShares"
                  "sharedWithMe" "listShared" "statShared" "readShared" "saveShared"
                  "listMail" "statMail" "readMail" "shareEntries" "shareDownload"
                  "shareExpired" "shareRequiresPassword" "shareInfo" "recover" "dispose"]]
      (aset result name
            (fn [& args]
              (try
                (let [value (.apply (aget engine name) engine (into-array (map to-js args)))]
                  (if (and value (fn? (.-then value)))
                    (-> value (.then from-js)
                        (.catch (fn [error] (throw (translate-error error)))))
                    (from-js value)))
                (catch :default error (throw (translate-error error)))))))
    result))

(defn service
  ([data-root options database atomic-store limiter keyring]
   (service data-root options database atomic-store limiter keyring nil nil))
  ([data-root options database atomic-store limiter keyring search]
   (service data-root options database atomic-store limiter keyring search nil))
  ([data-root {:keys [quota-bytes]} ^js database ^js atomic-store
    ^js limiter ^js keyring ^js search changed!]
   (let [host #js {:dataRoot data-root :quotaBytes (or quota-bytes 0)
                   :connection (.-connection database)
                   :write (fn [operation] (.write (.-writer database) operation))
                   :normalizePath path/normalize-user-path
                   :verifyPath path/verify-no-symlinks!
                   :readBounded path/read-bounded!
                   :atomicWrite (fn [root relative bytes]
                                  (atomic-files/write-file! atomic-store root relative bytes))
                   :journal (fn [operation payload user-id]
                              (-> (.journal atomic-store
                                            {:operation operation :payload (from-js payload)
                                             :user-id user-id})
                                  (.then clj->js)))
                   :markReconciled (fn [id] (.markReconciled atomic-store id))
                   :registerReconciler (fn [operation handler]
                                         (.registerReconciler atomic-store operation
                                                              (fn [payload] (handler (clj->js payload)))))
                   :reconcileJournal (fn [] (.reconcile atomic-store))
                   :allow (fn [key maximum window-ms] (.allow limiter key maximum window-ms))
                   :mac (fn [purpose input] (.mac keyring purpose input))}]
     (when changed! (aset host "changed" changed!))
     (when search
       (aset host "index" (fn [user vpath title body url]
                            (.index search user "files" vpath title body url)))
       (aset host "unindex" (fn [user vpath] (.delete search user "files" vpath)))
       (aset host "reconcile" (fn [user documents]
                                (.reconcileProvider search user "files" (from-js documents) "share:")))
       (aset host "indexInTransaction"
             (fn [user ^js document]
               (search-index/index-in-transaction! (.-connection database) user "files"
                                                   (.-id document) (.-title document)
                                                   (.-body document) (.-url document))))
       (aset host "deleteInTransaction"
             (fn [user id] (search-index/delete-in-transaction! (.-connection database) user "files" id))))
     (wrap-engine (files-ts/createFiles host)))))
