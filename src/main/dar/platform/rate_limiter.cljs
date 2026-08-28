(ns dar.platform.rate-limiter
  (:require [dar.cordis :as cordis]))

;; Fixed-window in-memory rate limiting for abuse-prone endpoints. Windows are
;; per key; the caller owns the key shape, the limit, and the window length.
;; The clock is injectable so tests are deterministic without sleeping.

(defn service
  "Builds a rate limiter. `now-fn` returns the current time in milliseconds."
  [now-fn]
  (let [windows (js/Map.)]
    #js {:allow
         (fn [key limit window-ms]
           (let [now (now-fn)
                 current (.get windows key)
                 active (if (and (some? current)
                                 (< now (+ (.-windowStart current) window-ms)))
                          current
                          #js {:windowStart now :count 0 :windowMs window-ms})]
             (.set windows key active)
             (set! (.-count active) (+ (.-count active) 1))
             (if (<= (.-count active) limit)
               #js {:allowed true}
               #js {:allowed false
                    :retryAfterMs (- (+ (.-windowStart active) window-ms) now)})))
         :prune
         (fn []
           (let [now (now-fn)]
             (.forEach windows
                       (fn [^js entry key]
                         (when (>= now (+ (.-windowStart entry) (.-windowMs entry)))
                           (.delete windows key))))
             nil))
         :size
         (fn [] (.-size windows))}))

(defn prune-interval-ms [] 60000)

(def rate-limiter-plugin
  (cordis/plugin
   {:name "dar-rate-limiter"
    :inject []
    :apply
    (fn [^js ctx _config]
      (let [^js store (service (fn [] (.now js/Date)))
            ^js timer (js/setInterval (.-prune store) (prune-interval-ms))]
        (.unref timer)
        (cordis/effect! ctx "dar.rate-limiter.dispose"
                        (fn []
                          (fn [] (js/clearInterval timer))))
        (cordis/provide! ctx "rateLimiter" store)
        nil))}))
