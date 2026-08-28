(ns dar.platform.path
  (:require [clojure.string :as str]
            ["node:fs/promises" :as fs]
            ["node:path" :as path]))

(def reserved-segments
  #{".dar" ".trash" ".versions"})

(defn unsafe-path!
  ([message]
   (unsafe-path! message nil))
  ([message details]
   (throw (ex-info message (merge {:type :unsafe-path
                                   :code "unsafePath"}
                                  details)))))

(defn normalize-user-path
  "Returns a canonical slash-separated relative path or throws unsafePath."
  ([candidate]
   (normalize-user-path candidate false))
  ([candidate allow-empty?]
   (when-not (string? candidate)
     (unsafe-path! "A managed path must be a string."))
   (when (or (str/includes? candidate "\u0000")
             (str/includes? candidate "\\")
             (.isAbsolute path candidate))
     (unsafe-path! "The path contains an unsafe absolute path or separator."
                   {:path candidate}))
   (let [segments (str/split candidate #"/" -1)]
     (when (and (not allow-empty?) (= [""] segments))
       (unsafe-path! "The path cannot be empty." {:path candidate}))
     (doseq [segment segments]
       (when (or (str/blank? segment)
                 (= "." segment)
                 (= ".." segment)
                 (contains? reserved-segments segment))
         (unsafe-path! "The path contains an unsafe or reserved segment."
                       {:path candidate :segment segment})))
     (str/join "/" segments))))

(defn resolve-user-path [root candidate]
  (let [normalized (normalize-user-path candidate)
        resolved (.resolve path root normalized)
        prefix (str (.resolve path root) (.-sep path))]
    (when-not (str/starts-with? resolved prefix)
      (unsafe-path! "The path escapes managed storage." {:path candidate}))
    resolved))

(defn- missing? [error]
  (= "ENOENT" (.-code error)))

(defn verify-no-symlinks!
  "Checks every existing component beneath root and returns the resolved path.

  Missing trailing components are allowed so callers can safely validate create
  destinations before using atomic file operations."
  [root candidate]
  (let [normalized (normalize-user-path candidate)
        segments (str/split normalized #"/")
        target (resolve-user-path root candidate)]
    (.then
     (reduce
      (fn [promise segment]
        (.then promise
               (fn [{:keys [current missing]}]
                 (let [next (.resolve path current segment)]
                   (if missing
                     {:current next :missing true}
                     (-> (.lstat fs next)
                         (.then (fn [^js stat]
                                  (when (.isSymbolicLink stat)
                                    (unsafe-path! "Managed storage cannot contain symbolic links."
                                                  {:path candidate :component next}))
                                  {:current next :missing false}))
                         (.catch (fn [error]
                                   (if (missing? error)
                                     {:current next :missing true}
                                     (throw error))))))))))
      (-> (.lstat fs (.resolve path root))
          (.then (fn [^js stat]
                   (when (.isSymbolicLink stat)
                     (unsafe-path! "The managed storage root cannot be a symbolic link."
                                   {:path candidate :component root}))
                   {:current (.resolve path root) :missing false})))
      segments)
     (fn [_] target))))
