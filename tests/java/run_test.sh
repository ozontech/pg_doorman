#!/usr/bin/env bash
set -e

# Usage: ./run_test.sh <test_name> <source_file>
# Example: ./run_test.sh simple_select SimpleSelect.java

TEST_NAME=$1
SOURCE_FILE=$2

if [ -z "$TEST_NAME" ] || [ -z "$SOURCE_FILE" ]; then
    echo "Usage: $0 <test_name> <source_file>"
    exit 1
fi

# Get absolute path to data directory before changing directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="${SCRIPT_DIR}/data"

# Create temporary project directory
PROJECT_DIR=$(mktemp -d)
trap "rm -rf '${PROJECT_DIR}'" EXIT

cd "${PROJECT_DIR}"

# Create Maven pom.xml with Spring Boot and HikariCP
cat > pom.xml << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 http://maven.apache.org/xsd/maven-4.0.0.xsd">
    <modelVersion>4.0.0</modelVersion>

    <groupId>com.pgdoorman.test</groupId>
    <artifactId>java-test</artifactId>
    <version>1.0-SNAPSHOT</version>
    <packaging>jar</packaging>

    <properties>
        <maven.compiler.source>21</maven.compiler.source>
        <maven.compiler.target>21</maven.compiler.target>
        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
    </properties>

    <dependencies>
        <!-- PostgreSQL JDBC Driver -->
        <dependency>
            <groupId>org.postgresql</groupId>
            <artifactId>postgresql</artifactId>
            <version>42.7.4</version>
        </dependency>
        <!-- HikariCP Connection Pool -->
        <dependency>
            <groupId>com.zaxxer</groupId>
            <artifactId>HikariCP</artifactId>
            <version>6.2.1</version>
        </dependency>
        <!-- SLF4J for logging -->
        <dependency>
            <groupId>org.slf4j</groupId>
            <artifactId>slf4j-simple</artifactId>
            <version>2.0.16</version>
        </dependency>
    </dependencies>

    <build>
        <plugins>
            <plugin>
                <groupId>org.codehaus.mojo</groupId>
                <artifactId>exec-maven-plugin</artifactId>
                <version>3.5.0</version>
                <configuration>
                    <mainClass>Main</mainClass>
                </configuration>
            </plugin>
        </plugins>
    </build>
</project>
EOF

# Create source directory structure
mkdir -p src/main/java

# Copy test source
cp -f "${DATA_DIR}/${SOURCE_FILE}" src/main/java/Main.java

# Run the test (compile and execute)
mvn -q compile exec:java
