const { Client } = require('pg');

const client = new Client({ connectionString: process.env.DATABASE_URL });

// Connect to the database
client
	.connect()
	.then(() => {
		console.log('Connected to PostgreSQL database');

		// Execute SQL queries here

		client.query('drop table if exists node_users');
		client.query('create table node_users (id serial primary key, name text)');
		client.query('insert into node_users(name) values ($1)', ['Dima']);
		client.query('select * from node_users where name = $1', ['Dima']);
		client.query('select * from node_users', (err, _) => { if (err) {
			console.error('Query error:', err);
			process.exit(1);
		}
			client
				.end()
				.then(() => {
					console.log('Connection to PostgreSQL closed');
					process.exit(0);
				})
				.catch((err) => {
					console.error('Error closing connection:', err);
					process.exit(1);
				});
		});
	})
	.catch((err) => {
		console.error('Error connecting to PostgreSQL database', err);
		process.exit(1);
	});