(ns dar.shared.contracts
  (:require [malli.core :as m]))

(def error-codes
  #{"invalid"
    "unauthenticated"
    "forbidden"
    "notFound"
    "conflict"
    "rateLimited"
    "tooLarge"
    "quotaExceeded"
    "unsupportedMediaType"
    "preconditionRequired"
    "preconditionFailed"
    "locked"
    "gone"
    "unavailable"
    "internal"})

(def error-envelope-schema
  [:map
   [:error
    [:map
     [:code [:enum
             "invalid"
             "unauthenticated"
             "forbidden"
             "notFound"
             "conflict"
             "rateLimited"
             "tooLarge"
             "quotaExceeded"
             "unsupportedMediaType"
             "preconditionRequired"
             "preconditionFailed"
             "locked"
             "gone"
             "unavailable"
             "internal"]]
     [:message string?]
     [:fields {:optional true} map?]
     [:requestId string?]]]])

(def collection-schema
  [:map
   [:items [:vector any?]]
   [:nextCursor {:optional true} [:maybe string?]]])

(defn valid-error-envelope? [value]
  (m/validate error-envelope-schema value))

(defn error-envelope
  ([code message request-id]
   (error-envelope code message request-id nil))
  ([code message request-id fields]
   {:pre [(contains? error-codes code)]}
   {:error (cond-> {:code code
                    :message message
                    :requestId request-id}
             fields (assoc :fields fields))}))

(defn collection
  ([items]
   (collection items nil))
  ([items next-cursor]
   (cond-> {:items (vec items)}
     next-cursor (assoc :nextCursor next-cursor))))

