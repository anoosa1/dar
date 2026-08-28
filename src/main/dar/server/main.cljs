(ns dar.server.main
  (:require [dar.cordis :as cordis]
            [dar.platform.auth :as auth]
            [dar.platform.contributions :as contributions]
            [dar.platform.database :as database]
            [dar.platform.instance :as instance]
            [dar.platform.keyring :as keyring]
            [dar.platform.oidc :as oidc]
            [dar.platform.preferences :as preferences]
            [dar.platform.security :as security]
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
         (.then (fn []
                  (cordis/mount! ctx oidc/oidc-plugin runtime-config)))
         (.then (fn []
                  (cordis/mount! ctx preferences/preferences-plugin)))
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
