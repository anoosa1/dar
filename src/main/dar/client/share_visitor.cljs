(ns dar.client.share-visitor
  (:require ["dar-files/share-visitor" :refer [ShareVisitor]]))

(defn page [token]
  [:> ShareVisitor {:token token}])
