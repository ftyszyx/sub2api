# pull new server image
docker compose -f docker-compose.zyx.yml pull server
# stop server
docker compose -f docker-compose.zyx.yml down server
# start server
docker compose -f docker-compose.zyx.yml up -d --force-recreate server
