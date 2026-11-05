(ns dar.platform.security-test
  (:require [cljs.test :refer [async deftest is testing]]
            [dar.cordis :as cordis]
            [dar.platform.security :as security]))

(deftest identifies-mutating-methods
  (is (security/unsafe-method? "POST"))
  (is (security/unsafe-method? "delete"))
  (is (not (security/unsafe-method? "GET"))))

(deftest enforces-origin-on-browser-api-mutations
  (let [expected "https://dar.example"]
    (is (security/request-origin-allowed?
         "POST" "/api/v1/files" expected expected))
    (is (not (security/request-origin-allowed?
              "POST" "/api/v1/files" nil expected)))
    (is (not (security/request-origin-allowed?
              "DELETE" "/api/v1/files/1" "https://evil.example" expected)))
    (testing "safe API reads and protocol endpoints do not require Origin"
      (is (security/request-origin-allowed? "GET" "/api/v1/files" nil expected))
      (is (security/request-origin-allowed? "PUT" "/dav/files/user/a" nil expected)))))

(deftest middleware-enforces-limits-before-routing-and-keeps-bounded-upload-exceptions
  (async done
    (let [ctx (cordis/context)
          invoke (fn [method path origin length]
                   (let [body (atom nil) routed (atom false)
                         request #js {:method method :path path :headers (js/Map. (clj->js (cond-> [] origin (conj ["origin" origin]) (some? length) (conj ["content-length" length]))))}
                         response #js {:headers (js/Map.) :json #(reset! body (js->clj % :keywordize-keys true))}]
                     (.handle (.-httpSecurity ctx) request response #(reset! routed true))
                     (is (string? (.-requestId request)))
                     (is (= (.-requestId request) (.get (.-headers response) "x-request-id")))
                     (doseq [[header value] security/security-headers]
                       (is (= value (.get (.-headers response) header))))
                     {:body @body :status (.-status response) :routed @routed :cache (.get (.-headers response) "cache-control")}))]
      (cordis/provide! ctx "server" #js {:baseUrl "https://dar.example"})
      (-> (cordis/mount! ctx security/security-plugin {:bodyLimit 32})
          (.then (fn [_]
                   (testing "API mutation origins and ordinary body limits apply before routing"
                     (let [denied (invoke "POST" "/api/v1/todo/lists" nil nil)]
                       (is (= 403 (:status denied))) (is (= "forbidden" (get-in denied [:body :error :code])))
                       (is (not (:routed denied))) (is (= "no-store" (:cache denied))))
                     (doseq [length ["33" "-1" "1.5" "invalid" "9007199254740992"]]
                       (let [denied (invoke "POST" "/api/v1/todo/lists" "https://dar.example" length)]
                         (is (= 413 (:status denied))) (is (not (:routed denied)))))
                     (is (:routed (invoke "POST" "/api/v1/todo/lists" "https://dar.example" "32")))
                     (is (:routed (invoke "GET" "/api/v1/files" nil nil))))
                   (testing "only explicitly bounded upload routes may exceed the generic body limit"
                     (doseq [path ["/api/v1/files/upload" "/api/v1/email/attachments" "/api/v1/news/articles"
                                   "/api/v1/notes/opaque/attachments" "/api/v1/chat/conversations/opaque/attachments"]]
                       (is (:routed (invoke "POST" path "https://dar.example" "100")))
                       (is (= 403 (:status (invoke "POST" path "https://other.example" "100")))))
                     (is (= 413 (:status (invoke "POST" "/api/v1/notes/opaque/attachments/extra" "https://dar.example" "100")))))
                   (testing "protocol requests remain outside browser Origin rules"
                     (let [protocol (invoke "PUT" "/dav/tasks/user/list/task.ics" nil "32")]
                       (is (:routed protocol)) (is (nil? (:cache protocol)))))))
          (.catch #(is false (str %)))
          (.finally (fn [] (-> (cordis/dispose! ctx) (.finally done))))))))
