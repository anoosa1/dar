(ns dar.cordis-test
  (:require [cljs.test :refer-macros [async deftest is]]
            [dar.cordis :as cordis]))

(deftest effects-dispose-with-their-plugin
  (async done
    (let [events (atom [])
          ctx (cordis/context)
          test-plugin
          (cordis/plugin
           {:name "dar-test-plugin"
            :apply (fn [plugin-ctx _config]
                     (cordis/effect!
                      plugin-ctx
                      "dar.test.effect"
                      (fn []
                        (swap! events conj :started)
                        #(swap! events conj :stopped))))})]
      (-> (cordis/mount! ctx test-plugin)
          (.then (fn []
                   (is (= [:started] @events))
                   (cordis/dispose! ctx)))
          (.then (fn []
                   (is (= [:started :stopped] @events))
                   (done)))
          (.catch (fn [error]
                    (is false (str error))
                    (done)))))))

(deftest mount-waits-for-asynchronous-plugin-activation
  (async done
    (let [activated? (atom false)
          ctx (cordis/context)
          test-plugin
          (cordis/plugin
           {:name "dar-async-test-plugin"
            :apply (fn [_ctx _config]
                     (js/Promise.
                      (fn [resolve _reject]
                        (js/setTimeout
                         (fn []
                           (reset! activated? true)
                           (resolve nil))
                         10))))})]
      (-> (cordis/mount! ctx test-plugin)
          (.then (fn []
                   (is (true? @activated?))
                   (cordis/dispose! ctx)))
          (.then done)
          (.catch (fn [error]
                    (is false (str error))
                    (done)))))))
