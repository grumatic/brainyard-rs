;; Copyright (c) 2024-2026 Grumatic, Inc.
;; SPDX-License-Identifier: MIT
;; Licensed under the MIT License. See LICENSE at the repository root.

(ns ai.brainyard.agent-tui-app.dotenv-test
  (:require
   [clojure.java.io :as io]
   [clojure.test :refer [deftest is testing]]
   [ai.brainyard.agent-tui-app.dotenv :as dotenv]))

(defn- set-property! [k v]
  (if (nil? v)
    (System/clearProperty k)
    (System/setProperty k v)))

(defn- with-system-properties [props f]
  (let [sentinel (Object.)
        prior (reduce-kv (fn [acc k _]
                           (assoc acc k (or (System/getProperty k) sentinel)))
                         {}
                         props)]
    (try
      (doseq [[k v] props]
        (set-property! k v))
      (f)
      (finally
        (doseq [[k v] prior]
          (if (identical? sentinel v)
            (System/clearProperty k)
            (System/setProperty k v)))))))

(deftest load-from-dotenv-honors-opt-out
  (testing "BY_NO_DOTENV disables .env discovery"
    (let [root (doto (io/file (System/getProperty "java.io.tmpdir")
                              (str "by-dotenv-test-" (System/nanoTime)))
                 (.mkdirs))
          home (io/file root "home")
          cwd  (io/file root "work")
          key  "BY_RS_DOTENV_SHOULD_NOT_LOAD"]
      (.mkdirs home)
      (.mkdirs cwd)
      (spit (io/file cwd ".env") (str key "=secret\n"))
      (System/clearProperty key)
      (with-system-properties {"user.dir" (.getAbsolutePath cwd)
                               "user.home" (.getAbsolutePath home)
                               "BY_NO_DOTENV" "1"}
        #(let [result (dotenv/load-from-dotenv!)]
           (is (= {:paths [] :loaded-count 0} result))
           (is (nil? (System/getProperty key))))))))
