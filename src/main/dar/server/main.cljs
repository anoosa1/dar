(ns dar.server.main
  (:require [dar.cordis :as cordis]
            [dar.apps.calendar-http :as calendar-http]
            [dar.apps.contacts-http :as contacts-http]
            [dar.apps.activity-http :as activity-http]
            [dar.apps.chat-http :as chat-http]
            [dar.apps.todo-http :as todo-http]
            [dar.apps.notes-http :as notes-http]
            [dar.apps.prayer-http :as prayer-http]
            [dar.apps.weather-http :as weather-http]
            [dar.apps.news-http :as news-http]
            [dar.apps.email-http :as email-http]
            [dar.apps.files-http :as files-http]
            [dar.platform.activity :as activity]
            [dar.platform.app-passwords :as app-passwords]
            [dar.platform.assistant :as assistant]
            [dar.platform.dashboard :as dashboard]
            [dar.platform.atomic-files :as atomic-files]
            [dar.platform.outbound :as outbound]
            [dar.platform.rate-limiter :as rate-limiter]
            [dar.platform.auth :as auth]
            [dar.platform.contributions :as contributions]
            [dar.platform.database :as database]
            [dar.platform.instance :as instance]
            [dar.platform.keyring :as keyring]
            [dar.platform.oidc :as oidc]
            [dar.platform.notifications :as notifications]
            [dar.platform.preferences :as preferences]
            [dar.platform.personal :as personal]
            [dar.platform.appearance :as appearance]
            [dar.platform.processes :as processes]
            [dar.platform.events :as events]
            [dar.platform.security :as security]
            [dar.platform.search :as search]
            [dar.server.config :as config]
            [dar.server.core :as core]
            ["@cordisjs/plugin-logger-console" :default ConsoleExporter]
            ["@cordisjs/plugin-server" :default Server]
            ["node:url" :refer [pathToFileURL]]))

(defonce running-context (atom nil))

(defn- server-config [{:keys [host port baseUrl]}]
  (cond-> {:host host
           :port port
           :maxPort port}
    baseUrl (assoc :baseUrl baseUrl)))

(defn- install-signals! [ctx]
  (cordis/effect!
   ctx
   "dar.process-signals"
   (fn []
     (let [stopping? (atom false)
           stop! (fn [signal]
                   (when (compare-and-set! stopping? false true)
                     (js/console.info (str "received " signal ", stopping Dar"))
                     (-> (cordis/dispose! ctx)
                         (.then (fn [] (set! (.-exitCode js/process) 0)))
                         (.catch (fn [error]
                                   (js/console.error error)
                                   (set! (.-exitCode js/process) 1))))))]
       (.on js/process "SIGINT" stop!)
       (.on js/process "SIGTERM" stop!)
       (fn []
         (.off js/process "SIGINT" stop!)
         (.off js/process "SIGTERM" stop!))))))

(defn start!
  ([]
   (start! (config/from-env (.-env js/process))))
  ([runtime-config]
   (let [ctx (cordis/context)
         previous-umask (.umask js/process 63)]
     (cordis/effect! ctx "dar.process-umask"
                     (fn []
                       (fn [] (.umask js/process previous-umask))))
     (set! (.-baseUrl ctx) (.-href (pathToFileURL (str (.cwd js/process) "/"))))
     (-> (cordis/mount! ctx ConsoleExporter {:showTime "yyyy-MM-dd hh:mm:ss.SSS"})
         (.then (fn []
                  (cordis/mount! ctx contributions/contributions-plugin)))
         (.then (fn [] (cordis/mount! ctx processes/processes-plugin)))
         (.then (fn []
                  (cordis/mount! ctx instance/instance-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx database/database-plugin)))
         (.then (fn []
                  (cordis/mount! ctx keyring/keyring-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx Server (server-config runtime-config))))
         (.then (fn []
                  (cordis/mount! ctx security/security-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx auth/auth-plugin runtime-config)))
         (.then (fn [] (cordis/mount! ctx events/events-plugin)))
         (.then (fn []
                   (cordis/mount! ctx app-passwords/app-passwords-plugin)))
         (.then (fn []
                  (cordis/mount! ctx atomic-files/atomic-files-plugin)))
         (.then (fn []
                  (cordis/mount! ctx rate-limiter/rate-limiter-plugin)
  (cordis/mount! ctx outbound/outbound-plugin)))
         (.then (fn []
                  (cordis/mount! ctx assistant/assistant-plugin)))
         (.then (fn []
                  (cordis/mount! ctx oidc/oidc-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx preferences/preferences-plugin)))
         (.then (fn []
                  (cordis/mount! ctx dashboard/dashboard-plugin)))
         (.then (fn []
                  (cordis/mount! ctx notifications/notifications-plugin)))
         (.then (fn []
                  (cordis/mount! ctx activity/activity-plugin)))
         (.then (fn [] (cordis/mount! ctx activity-http/activity-app-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx search/search-plugin)))
         (.then (fn []
                  (cordis/mount! ctx files-http/files-plugin runtime-config)))
         (.then (fn [] (cordis/mount! ctx chat-http/chat-plugin runtime-config)))
         (.then (fn [] (cordis/mount! ctx notes-http/notes-plugin runtime-config)))
          (.then (fn []
                   (cordis/mount! ctx calendar-http/calendar-plugin runtime-config)))
          (.then (fn []
                   (cordis/mount! ctx contacts-http/contacts-plugin runtime-config)))
          (.then (fn [] (cordis/mount! ctx personal/personal-plugin)))
          ;; Todo's widget consumes profile formatting. Mount after Personal so
          ;; its portable recovery finishes before Prayer and readiness.
          (.then (fn [] (cordis/mount! ctx todo-http/todo-plugin runtime-config)))
          (.then (fn [] (cordis/mount! ctx appearance/appearance-plugin)))
          (.then (fn [] (cordis/mount! ctx prayer-http/prayer-plugin)))
          (.then (fn [] (cordis/mount! ctx weather-http/weather-plugin)))
          (.then (fn [] (cordis/mount! ctx news-http/news-plugin)))
          (.then (fn [] (cordis/mount! ctx email-http/email-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx core/core-plugin runtime-config)))
         (.then (fn []
                  (when (:installSignals runtime-config)
                    (install-signals! ctx))
                  (reset! running-context ctx)
                  ctx))
         (.catch (fn [error]
                   (-> (cordis/dispose! ctx)
                       (.catch (fn [dispose-error]
                                 (js/console.error dispose-error)))
                       (.then (fn []
                                (throw error))))))))))

(defn stop! []
  (if-let [ctx @running-context]
    (do
      (reset! running-context nil)
      (cordis/dispose! ctx))
    (js/Promise.resolve nil)))

(defn main []
  (-> (start!)
      (.then (fn [^js ctx]
               (let [^js server (.-server ctx)]
                 (js/console.info (str "Dar is ready at " (.-baseUrl server))))))
      (.catch (fn [error]
                (js/console.error error)
                (set! (.-exitCode js/process) 1)))))
