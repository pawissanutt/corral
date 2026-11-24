-- CREATE TABLE fictional (id INT);
CREATE TABLE app."Customers" (
    id BIGINT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS app.orders (
    product VARCHAR /* trivia */ ( 32 ) NOT NULL DEFAULT 'with, comma',
    customer_id BIGINT REFERENCES app."Customers"(id),
    reviewed_by BIGINT,
    CONSTRAINT reviewer FOREIGN KEY (reviewed_by) REFERENCES app."Customers"(id)
);
