//go:build unit

package service

import (
	"context"
	"time"
)

type responsesChatStateCacheStub struct{}

func (responsesChatStateCacheStub) GetResponsesChatState(context.Context, int64, string) ([]byte, error) {
	return nil, errResponsesChatPreviousNotFound
}

func (responsesChatStateCacheStub) SetResponsesChatState(context.Context, int64, string, []byte, time.Duration) error {
	return nil
}

func (responsesChatStateCacheStub) DeleteResponsesChatState(context.Context, int64, string) error {
	return nil
}
