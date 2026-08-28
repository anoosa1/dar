(ns dar.platform.assistant-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.assistant :as assistant]
            [dar.platform.contributions :as contributions]))

(def ^:private invoked (atom []))

(defn- fresh-registry []
  (reset! invoked [])
  (let [^js registry (contributions/registry)]
    (assistant/register-tool!
     registry "test.echo"
     {:name "test.echo"
      :title "Echo the payload"
      :effect "read"
      :schema [:map {:closed true} [:payload string?]]
      :invoke (fn [_user arguments]
                (swap! invoked conj arguments)
                (js/Promise.resolve {:echo (:payload arguments)}))})
    (assistant/register-tool!
     registry "test.destroy"
     {:name "test.destroy"
      :title "Destroy things"
      :effect "destructive"
      :schema [:map {:closed true} [:target string?]]
      :invoke (fn [_user _arguments]
                (js/Promise.resolve {:destroyed true}))})
    registry))

(deftest rejects-invalid-tool-descriptors
  (let [^js registry (contributions/registry)]
    (is (thrown-with-msg? js/Error #"dotted and namespaced"
                          (assistant/register-tool! registry "echo" {:invoke (fn [])})))
    (is (thrown-with-msg? js/Error #"needs a namespaced name"
                          (assistant/register-tool! registry "test.broken"
                                                    {:name "test.broken"
                                                     :title ""
                                                     :effect "read"
                                                     :schema [:map]
                                                     :invoke (fn [])})))
    (is (thrown-with-msg? js/Error #"needs a namespaced name"
                          (assistant/register-tool! registry "test.broken"
                                                    {:name "test.broken"
                                                     :title "Broken"
                                                     :effect "explode"
                                                     :schema [:map]
                                                     :invoke (fn [])})))
    (is (thrown-with-msg? js/Error #"must match its contribution id"
                          (assistant/register-tool! registry "test.broken"
                                                    {:name "test.other"
                                                     :title "Broken"
                                                     :effect "read"
                                                     :schema [:map]
                                                     :invoke (fn [])})))
    (is (thrown? js/Error
                 (assistant/register-tool! registry "test.bad"
                                           {:name "test.bad"
                                            :title "Bad"
                                            :effect "read"
                                            :schema :not-a-schema
                                            :invoke (fn [])})))
    (is (thrown? js/Error
                 (assistant/register-tool! registry "test.schema"
                                           {:name "test.schema"
                                            :title "Bad schema"
                                            :effect "read"
                                            :schema :not-a-schema
                                            :invoke (fn [])})))))

(deftest catalog-exposes-name-title-effect-and-json-schema
  (let [^js registry (fresh-registry)
        items (vec (assistant/list-tools registry))
        echo (first (filter (fn [tool] (= "test.echo" (:name tool))) items))
        parameters (:parameters echo)]
    (is (= 2 (count items)))
    (is (= "Echo the payload" (:title echo)))
    (is (= "read" (:effect echo)))
    (is (= "object" (:type parameters)))
    (is (contains? (set (:required parameters)) :payload))))

(deftest invocation-runs-validated-tools-with-user-scope
  (async done
    (let [^js registry (fresh-registry)
          ^js user #js {:id "user-1"}]
      (-> (assistant/invoke registry user "test.echo" {:payload "hi"} false)
          (.then (fn [outcome]
                   (is (= {:echo "hi"} (:result outcome)))
                   (is (= [{:payload "hi"}] @invoked))))
          (.then (fn [_] (done)))
          (.catch (fn [error]
                    (is false (str "invocation failed: " (.-message error)))
                    (done)))))))

(deftest invocation-rejects-unknown-tools-and-invalid-arguments
  (async done
    (let [^js registry (fresh-registry)
          ^js user #js {:id "user-1"}]
      (-> (assistant/invoke registry user "test.missing" {} false)
          (.then (fn [outcome]
                   (is (= :tool-not-found (:error outcome)))))
          (.then (fn [_]
                   (assistant/invoke registry user "test.echo" {:payload 7} false)))
          (.then (fn [outcome]
                   (is (= :invalid-arguments (:error outcome)))
                   (is (some? (:fields outcome)))))
          (.then (fn [_] (done)))
          (.catch (fn [error]
                    (is false (str "invocation failed: " (.-message error)))
                    (done)))))))

(deftest destructive-tools-require-explicit-confirmation
  (async done
    (let [^js registry (fresh-registry)
          ^js user #js {:id "user-1"}]
      (-> (assistant/invoke registry user "test.destroy" {:target "a"} false)
          (.then (fn [outcome]
                   (is (= :confirmation-required (:error outcome)))
                   (is (= "destructive" (:effect outcome)))))
          (.then (fn [_]
                   (assistant/invoke registry user "test.destroy" {:target "a"} true)))
          (.then (fn [outcome]
                   (is (= {:destroyed true} (:result outcome)))))
          (.then (fn [_] (done)))
          (.catch (fn [error]
                    (is false (str "confirmation flow failed: " (.-message error)))
                    (done)))))))

(deftest tool-failures-carry-stable-categories
  (async done
    (let [^js registry (contributions/registry)]
      (assistant/register-tool!
       registry "test.failing"
       {:name "test.failing"
        :title "Always fails"
        :effect "read"
        :schema [:map {:closed true}]
        :invoke (fn [_user _arguments]
                  (js/Promise.reject
                   (ex-info "Not found" {:type :not-found})))})
      (-> (assistant/invoke registry #js {:id "user-1"} "test.failing" {} false)
          (.then (fn [outcome]
                   (is (= :tool-failed (:error outcome)))
                   (is (= :not-found (:type (ex-data (:cause outcome)))))))
          (.then (fn [_] (done)))
          (.catch (fn [error]
                    (is false (str "failure mapping failed: " (.-message error)))
                    (done)))))))
