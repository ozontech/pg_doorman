require 'pg'
require 'json'
require 'tempfile'
require 'fileutils'
require 'securerandom'
require 'toml'

class ConfigReloadFailed < StandardError; end
class PgDoormanProcess
  attr_reader :port
  attr_reader :pid

  def self.finalize(pid, log_filename, config_filename)
    if pid
      Process.kill("TERM", pid)
      Process.wait(pid)
    end

    File.delete(config_filename) if File.exist?(config_filename)
    File.delete(log_filename) if File.exist?(log_filename)
  end

  def initialize(log_level)
    @env = {}
    @port = rand(20000..32760)
    @log_level = log_level
    @log_filename = "/tmp/pg_doorman_log_#{SecureRandom.urlsafe_base64}.log"
    @config_filename = "/tmp/pg_doorman_cfg_#{SecureRandom.urlsafe_base64}.toml"

    command_path = if ENV['CARGO_TARGET_DIR'] then
                     "#{ENV['CARGO_TARGET_DIR']}/debug/pg_doorman"
                   else
                     '../pg_doorman'
                   end

    @command = "#{command_path} #{@config_filename} --log-level #{@log_level}"

    FileUtils.cp("../../tests/tests.toml", @config_filename)
    cfg = current_config
    cfg["general"]["port"] = @port.to_i
    cfg["general"]["tls_private_key"] = nil
    cfg["general"]["tls_certificate"] = nil
    cfg["include"]["files"] = nil

    update_config(cfg)
    #print(raw_config_file)
  end

  def logs
    File.read(@log_filename)
  end

  def update_config(config_hash)
    @original_config = current_config
    File.write(@config_filename, TOML::Generator.new(config_hash).body)
  end

  def current_config
    TOML.load(File.read(@config_filename))
  end

  def raw_config_file
    File.read(@config_filename)
  end

  def reload_config
    conn = PG.connect(admin_connection_string)

    conn.async_exec("RELOAD")
  rescue PG::ConnectionBad => e
    errors = logs.split("Reloading config").last
    errors = errors.gsub(/\e\[([;\d]+)?m/, '') # Remove color codes
    errors = errors.
                split("\n").select{|line| line.include?("ERROR") }.
                map { |line| line.split("pg_doorman::config: ").last }
    raise ConfigReloadFailed, errors.join("\n")
  ensure
    conn&.close
  end

  def start
    raise StandardError, "Process is already started" unless @pid.nil?
    @pid = Process.spawn(@env, @command, err: @log_filename, out: @log_filename)
    Process.detach(@pid)
    ObjectSpace.define_finalizer(@log_filename, proc { PgDoormanProcess.finalize(@pid, @log_filename, @config_filename) })

    return self
  end

  def wait_until_ready(connection_string = nil)
    exc = nil
    10.times do
      Process.kill 0, @pid
      PG::connect(connection_string || example_connection_string).close

      return self
    rescue Errno::ESRCH
      raise StandardError, "Process #{@pid} died. #{logs}"
    rescue => e
      exc = e
      sleep(0.5)
    end
    puts exc
    raise StandardError, "Process #{@pid} never became ready. Logs #{logs}"
  end

  def stop
    return unless @pid

    Process.kill("TERM", @pid)
    Process.wait(@pid)
    @pid = nil
  end

  def log
    File.read(@log_filename) if File.exist?(@log_filename)
  end

  def shutdown
    stop
    File.delete(@config_filename) if File.exist?(@config_filename)
    File.delete(@log_filename) if File.exist?(@log_filename)
  end

  def admin_connection_string
    cfg = current_config
    username = cfg["general"]["admin_username"]
    password = cfg["general"]["admin_password"]

    "postgresql://#{username}:#{password}@0.0.0.0:#{@port}/pgbouncer"
  end

  def connection_string(pool_name, username, password = nil, parameters: {})
    cfg = current_config
    user_idx, user_obj = cfg["pools"][pool_name]["users"].detect { |k, user| user["username"] == username }
    connection_string = "postgresql://#{username}:#{password || user_obj["password"]}@0.0.0.0:#{@port}/#{pool_name}"

    # Add the additional parameters to the connection string
    parameter_string = parameters.map { |key, value| "#{key}=#{value}" }.join("&")
    connection_string += "?#{parameter_string}" unless parameter_string.empty?

    connection_string
  end

  def example_connection_string
    cfg = current_config
    first_pool_name = cfg["pools"].keys[0]

    db_name = first_pool_name

    username = cfg["pools"][first_pool_name]["users"]["0"]["username"]
    password = "test"

    "postgresql://#{username}:#{password}@0.0.0.0:#{@port}/#{db_name}?application_name=example_app"
  end
end