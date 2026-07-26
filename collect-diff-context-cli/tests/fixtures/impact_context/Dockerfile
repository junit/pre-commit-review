FROM rust:1.88
ENV APP_ENV=production
EXPOSE 8080
HEALTHCHECK CMD curl --fail http://localhost:8080/health
CMD ["./service"]
