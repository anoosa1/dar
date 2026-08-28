(ns dar.platform.rate-limiter-test
  (:require [cljs.test :refer [deftest is]]
            [dar.platform.rate-limiter :as rate-limiter]))

(defn- allowed? [^js result]
  (.-allowed result))

(defn- retry-after-ms [^js result]
  (.-retryAfterMs result))

(defn- fixed-clock
  "Returns [now-fn advance-fn] backed by a mutable counter."
  []
  (let [now (atom 1000)]
    [(fn [] @now)
     (fn [ms] (swap! now + ms))]))

(deftest allows-up-to-the-limit-then-requires-a-wait
  (let [[now advance] (fixed-clock)
        ^js limiter (rate-limiter/service now)
        ^js allow (.-allow limiter)]
    (is (true? (allowed? (allow "key" 2 60000))))
    (is (true? (allowed? (allow "key" 2 60000))))
    (let [denied (allow "key" 2 60000)]
      (is (false? (allowed? denied)))
      (is (= 60000 (retry-after-ms denied))))
    (advance 59999)
    (is (false? (allowed? (allow "key" 2 60000))))
    (advance 1)
    (is (true? (allowed? (allow "key" 2 60000))))))

(deftest keys-are-independent
  (let [[now _] (fixed-clock)
        ^js limiter (rate-limiter/service now)
        ^js allow (.-allow limiter)]
    (is (true? (allowed? (allow "share-unlock:user-1" 1 60000))))
    (is (true? (allowed? (allow "share-unlock:user-2" 1 60000))))
    (is (false? (allowed? (allow "share-unlock:user-1" 1 60000))))))

(deftest windows-are-per-call
  (let [[now advance] (fixed-clock)
        ^js limiter (rate-limiter/service now)
        ^js allow (.-allow limiter)]
    (is (true? (allowed? (allow "drop" 1 1000))))
    (is (true? (allowed? (allow "other" 1 5000))))
    (advance 1000)
    ;; "drop" rolled into a fresh window; "other" is still inside its own.
    (is (true? (allowed? (allow "drop" 1 1000))))
    (is (false? (allowed? (allow "other" 1 5000))))))

(deftest retry-after-shrinks-as-the-window-elapses
  (let [[now advance] (fixed-clock)
        ^js limiter (rate-limiter/service now)
        ^js allow (.-allow limiter)]
    (is (true? (allowed? (allow "key" 1 60000))))
    (is (= 60000 (retry-after-ms (allow "key" 1 60000))))
    (advance 20000)
    (is (= 40000 (retry-after-ms (allow "key" 1 60000))))))

(deftest prunes-expired-windows
  (let [[now advance] (fixed-clock)
        ^js limiter (rate-limiter/service now)
        ^js allow (.-allow limiter)
        ^js prune (.-prune limiter)
        ^js size (.-size limiter)]
    (allow "expired" 1 1000)
    (allow "active" 1 60000)
    (advance 2000)
    (prune)
    (is (= 1 (size)))
    (allow "active" 1 60000)
    (prune)
    (is (= 1 (size)))))

(deftest prune-keeps-unexpired-windows
  (let [[now _] (fixed-clock)
        ^js limiter (rate-limiter/service now)
        ^js allow (.-allow limiter)
        ^js prune (.-prune limiter)
        ^js size (.-size limiter)]
    (allow "fresh" 1 60000)
    (prune)
    (is (= 1 (size)))))
