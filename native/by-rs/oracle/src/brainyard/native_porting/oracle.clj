(ns brainyard.native-porting.oracle
  "Exports deterministic Clojure-side fixtures for the Rust port test harness."
  (:require
   [ai.brainyard.agent.interface :as agent]
   [ai.brainyard.clj-llm.interface :as clj-llm]
   [clojure.data.json :as json]
   [clojure.java.io :as io]
   [clojure.string :as str]))

(defn- scalar-name [value]
  (cond
    (nil? value) nil
    (keyword? value) (name value)
    (symbol? value) (name value)
    :else (str value)))

(defn- present-string [value]
  (let [s (scalar-name value)]
    (when-not (str/blank? (or s ""))
      s)))

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

(defn- registry-document []
  (array-map
   :agents (->> (agent/get-tool-defs :type :agent)
                (map agent-entry)
                (sort-by :id)
                vec)
   :models (->> (clj-llm/get-popular-models)
                (map model-entry)
                (sort-by (juxt :provider :id))
                vec)))

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
        (write-json-file! (io/file out-dir "registry.json") (registry-document))
        (write-json-file! (io/file out-dir "metadata.json")
                          (array-map :schemaVersion 1
                                     :exports ["registry.json"]
                                     :source "clojure"))
        (println (str "Wrote Clojure oracle fixtures to " (.getPath out-dir)))))))
