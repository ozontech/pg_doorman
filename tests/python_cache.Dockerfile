FROM python:3.8-slim

RUN apt-get update \
    && apt-get -y install libpq-dev gcc
COPY ./tests/python/requirements.txt ./
RUN pip install -r requirements.txt
