-- Create application database
CREATE DATABASE exampledb;

-- Create application users with passwords
-- pg_doorman will look these up via pg_shadow using auth_query
CREATE ROLE app_user LOGIN PASSWORD 'app_password';
CREATE ROLE readonly_user LOGIN PASSWORD 'readonly_pass';

-- Grant permissions
GRANT ALL ON DATABASE exampledb TO app_user;
GRANT CONNECT ON DATABASE exampledb TO readonly_user;
