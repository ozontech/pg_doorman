FROM golang:bookworm

RUN go env -w GOCACHE=/go-cache
RUN go env -w GOMODCACHE=/gomod-cache
COPY ./tests/go/go.* ./
RUN --mount=type=cache,target=/gomod-cache go mod download