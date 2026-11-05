(ns dar.platform.path
  (:require [clojure.string :as str]
            ["node:fs/promises" :as fs]
            ["node:fs" :refer [constants]]
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

(defn read-bounded!
  "Reads a regular managed file through one no-follow handle. Allocation stays
  bounded even if an external editor grows or replaces the file during reading."
  [root relative maximum]
  (when-not (and (int? maximum) (pos? maximum))
    (throw (ex-info "A positive file limit is required." {:type :invalid})))
  (-> (verify-no-symlinks! root relative)
      (.then (fn [full]
               (-> (.open fs full (bit-or (.-O_RDONLY constants) (.-O_NOFOLLOW constants)))
                   (.then (fn [^js handle]
                            (-> (.stat handle)
                                (.then (fn [^js before]
                                         (when-not (.isFile before)
                                           (throw (ex-info "Choose a regular file." {:type :invalid})))
                                         (when (> (.-size before) maximum)
                                           (throw (ex-info "The file exceeds the size limit." {:type :too-large})))
                                         (let [buffer (js/Buffer.alloc (inc (.-size before)))]
                                           (letfn [(read! [offset]
                                                     (if (= offset (.-length buffer)) (js/Promise.resolve offset)
                                                       (-> (.read handle buffer offset (- (.-length buffer) offset) offset)
                                                           (.then (fn [^js result]
                                                                    (if (zero? (.-bytesRead result)) offset
                                                                      (read! (+ offset (.-bytesRead result)))))))))]
                                             (-> (read! 0)
                                                 (.then (fn [length]
                                                          (-> (.lstat fs full)
                                                              (.then (fn [^js after]
                                                                       (when-not (and (.isFile after) (= length (.-size before))
                                                                                      (= (.-dev before) (.-dev after)) (= (.-ino before) (.-ino after))
                                                                                      (= (.-size before) (.-size after))
                                                                                      (= (.-mtimeMs before) (.-mtimeMs after))
                                                                                      (= (.-ctimeMs before) (.-ctimeMs after)))
                                                                         (throw (ex-info "The file changed during reading. Refresh and retry." {:type :conflict})))
                                                                       (.subarray buffer 0 length)))))))))))
                                (.finally #(.close handle))))))))))
