docker-compose-test-all: docker-compose-test-go docker-compose-test-dotnet docker-compose-test-nodejs docker-compose-test-python docker-compose-test-ruby
	echo ok

docker-compose-up:
	docker compose down --remove-orphans
	docker compose up -d
	sleep 2

docker-compose-test-go: docker-compose-up
	docker compose exec -T go_test /bin/bash -ec ' \
		cd /tests && source ./env && \
		go test -v .'

docker-compose-test-python: docker-compose-up
	docker compose exec -T python_test /bin/bash -ec ' \
		cd /tests && \
		python ./test_async.py && \
		python ./test_psycopg2.py && \
		python ./test_session_cursors.py'

docker-compose-test-nodejs: docker-compose-up
	docker compose exec -T nodejs_test /bin/bash -ec ' \
		cd /tests && npm install pg && \
		nodejs ./run.js'

docker-compose-test-ruby: docker-compose-up
	docker compose exec -T ruby_test /bin/bash -ec ' \
	  cd /tests && bundle config path ruby && cd ruby && bundle install && \
      bundle exec ruby test.rb && \
      install /usr/bin/pg_doorman /tests/ && \
      bundle exec rspec *_spec.rb'

docker-compose-test-dotnet: docker-compose-up
	docker compose exec -T dotnet_test /bin/bash -ec 'mkdir -p /tests/prj && cd /tests/prj && \
      rm -rf ./batch && mkdir -p ./batch && cd ./batch && dotnet new sln --name Batch && dotnet new console --output . && dotnet add package Npgsql && cp -av ../../data/batch.cs ./Program.cs && dotnet run Program.cs && \
      cd .. && rm -rf ./prepared && mkdir -p ./prepared && cd ./prepared && dotnet new sln --name Prepared && dotnet new console --output . && dotnet add package Npgsql && cp -av ../../data/prepared.cs ./Program.cs && dotnet run Program.cs && \
      cd .. && rm -rf ./pbde2 && mkdir -p ./pbde2 && cd ./pbde2 && dotnet new sln --name PBDE2 && dotnet new console --output . && dotnet add package Npgsql && cp -av ../../data/PBDE_PBDE_S.cs ./Program.cs && dotnet run Program.cs'
