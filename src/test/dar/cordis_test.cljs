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

