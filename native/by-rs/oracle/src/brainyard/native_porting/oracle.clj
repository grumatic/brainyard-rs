(ns brainyard.native-porting.oracle
  "Exports deterministic Clojure-side fixtures for the Rust port test harness."
  (:require
   [ai.brainyard.agent.interface :as agent]
   [ai.brainyard.agent.mcp.integration :as mcp]
   [ai.brainyard.clj-llm.interface :as clj-llm]
   [clojure.data.json :as json]
   [clojure.java.io :as io]
   [clojure.string :as str]))

(defn- scalar-name [value]
  (cond
    (nil? value) nil
    (keyword? value) (if-let [ns (namespace value)]
                       (str ns "/" (name value))
                       (name value))
    (symbol? value) (if-let [ns (namespace value)]
                      (str ns "/" (name value))
                      (name value))
    :else (str value)))

(defn- present-string [value]
  (let [s (scalar-name value)]
    (when-not (str/blank? (or s ""))
      s)))

(defn- json-safe-key [value]
  (or (present-string value) ""))

(defn- json-safe [value]
  (cond
    (nil? value) nil
    (or (string? value) (number? value) (boolean? value)) value
    (or (keyword? value) (symbol? value)) (scalar-name value)
    (map? value) (into (array-map)
                       (map (fn [[k v]]
                              [(json-safe-key k) (json-safe v)]))
                       value)
    (set? value) (->> value (map json-safe) (sort-by pr-str) vec)
    (sequential? value) (mapv json-safe value)
    :else (str value)))

(defn- agent-entry [[id spec]]
  (let [meta (:meta spec)
        agent-id (or (present-string id)
                     (present-string (:id spec))
                     (present-string (:id meta)))]
    (array-map
     :id agent-id
     :name (or (present-string (:name spec)) agent-id)
     :description (or (present-string (:description spec))
                      (present-string (:description meta))
                      "")
     :type (or (present-string (:type spec))
               (present-string (:type meta))
               "agent"))))

(defn- model-entry [spec]
  (cond-> (array-map
           :provider (present-string (:provider spec))
           :id (present-string (:model spec))
           :description (or (present-string (:description spec)) ""))
    (present-string (:region spec))
    (assoc :region (present-string (:region spec)))))

(defn- tool-entry [[id spec]]
  (let [meta (:meta spec)
        tool-id (or (present-string id)
                    (present-string (:id spec))
                    (present-string (:id meta)))
        tool-type (or (:type spec) (:type meta) :tool)]
    (cond-> (array-map
             :id tool-id
             :type (present-string tool-type)
             :description (or (present-string (:description spec))
                              (present-string (:description meta))
                              "")
             :inputSchema (json-safe (or (:input-schema meta) [:map]))
             :outputSchema (json-safe (or (:output-schema meta) [:map])))
      (seq (:aliases meta))
      (assoc :aliases (json-safe (:aliases meta)))
      (:tool-use-control meta)
      (assoc :toolUseControl (json-safe (:tool-use-control meta)))
      (:agent-tools meta)
      (assoc :agentTools (json-safe (:agent-tools meta)))
      (:config-schema meta)
      (assoc :configSchema (json-safe (:config-schema meta))))))

(defn- mcp-server-entry [[id spec]]
  (let [server-id (or (present-string id)
                      (present-string (:id spec))
                      (present-string (:name spec)))]
    (array-map
     :name server-id
     :transport (present-string (:transport spec))
     :config (json-safe (:config spec))
     :enabled (true? (:enabled spec))
     :autoRegisterTools (not (false? (:auto-register-tools spec))))))

(defn- registry-document []
  (let [tools (->> (agent/get-tool-defs)
                   (map tool-entry)
                   (sort-by (juxt :type :id))
                   vec)
        agents (->> (agent/get-tool-defs :type :agent)
                    (map agent-entry)
                    (sort-by :id)
                    vec)
        models (->> (clj-llm/get-popular-models)
                    (map model-entry)
                    (sort-by (juxt :provider :id))
                    vec)
        mcp-servers (->> (mcp/create-seed-mcp-config)
                         (map mcp-server-entry)
                         (sort-by :name)
                         vec)]
    (array-map
     :tools tools
     :agents agents
     :models models
     :mcpServers mcp-servers)))

(defn- write-json-file! [file value]
  (let [file (io/file file)]
    (.mkdirs (.getParentFile file))
    (spit file (str (json/write-str value :escape-slash false) "\n"))))

(defn- usage []
  (str "Usage: export-clojure-oracle [--out DIR]\n\n"
       "Writes deterministic fixtures consumed by native/by-rs tests."))

(defn- parse-args [args]
  (loop [remaining args
         opts {:out "native/by-rs/fixtures/oracle"}]
    (let [[arg value & more] remaining]
      (cond
        (nil? arg) opts
        (#{"--help" "-h"} arg) (assoc opts :help true)
        (= "--out" arg) (if value
                            (recur more (assoc opts :out value))
                            (throw (ex-info "--out requires a directory" {:arg arg})))
        :else (throw (ex-info (str "Unknown argument: " arg) {:arg arg}))))))

(defn -main [& args]
  (let [{:keys [out help]} (parse-args args)]
    (if help
      (println (usage))
      (let [out-dir (io/file out)]
        (.mkdirs out-dir)
        (let [registry (registry-document)]
          (write-json-file! (io/file out-dir "registry.json") registry)
          (write-json-file! (io/file out-dir "tools.json") (:tools registry))
          (write-json-file! (io/file out-dir "agents.json") (:agents registry))
          (write-json-file! (io/file out-dir "models.json") (:models registry))
          (write-json-file! (io/file out-dir "mcp-servers.json") (:mcpServers registry)))
        (write-json-file! (io/file out-dir "metadata.json")
                          (array-map :schemaVersion 1
                                     :exports ["registry.json"
                                               "tools.json"
                                               "agents.json"
                                               "models.json"
                                               "mcp-servers.json"]
                                     :source "clojure"))
        (println (str "Wrote Clojure oracle fixtures to " (.getPath out-dir)))))))
