/* -*- Mode: C++; tab-width: 2; indent-tabs-mode: nil; c-basic-offset: 2 -*- */
/* vim:set ts=2 sw=2 sts=2 et cindent: */
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

#include "mozilla/dom/BrowsingContext.h"
#include "mozilla/dom/ContentMediaController.h"
#include "mozilla/dom/MediaSession.h"
#include "mozilla/dom/MediaControlUtils.h"
#include "mozilla/dom/WindowContext.h"
#include "mozilla/EnumeratedArrayCycleCollection.h"

// avoid redefined macro in unified build
#undef LOG
#define LOG(msg, ...)                        \
  MOZ_LOG(gMediaControlLog, LogLevel::Debug, \
          ("MediaSession=%p, " msg, this, ##__VA_ARGS__))

namespace mozilla {
namespace dom {

// Only needed for refcounted objects.
NS_IMPL_CYCLE_COLLECTION_WRAPPERCACHE(MediaSession, mParent, mMediaMetadata,
                                      mActionHandlers)
NS_IMPL_CYCLE_COLLECTING_ADDREF(MediaSession)
NS_IMPL_CYCLE_COLLECTING_RELEASE(MediaSession)
NS_INTERFACE_MAP_BEGIN_CYCLE_COLLECTION(MediaSession)
  NS_WRAPPERCACHE_INTERFACE_MAP_ENTRY
  NS_INTERFACE_MAP_ENTRY(nsIDocumentActivity)
NS_INTERFACE_MAP_END

MediaSession::MediaSession(nsPIDOMWindowInner* aParent)
    : mParent(aParent), mDoc(mParent->GetExtantDoc()) {
  MOZ_ASSERT(mParent);
  MOZ_ASSERT(mDoc);
  mDoc->RegisterActivityObserver(this);
  if (mDoc->IsCurrentActiveDocument()) {
    SetMediaSessionDocStatus(SessionDocStatus::eActive);
  }
}

void MediaSession::Shutdown() {
  mDoc->UnregisterActivityObserver(this);
  SetMediaSessionDocStatus(SessionDocStatus::eInactive);
}

void MediaSession::NotifyOwnerDocumentActivityChanged() {
  const bool isDocActive = mDoc->IsCurrentActiveDocument();
  LOG("Document activity changed, isActive=%d", isDocActive);
  if (isDocActive) {
    SetMediaSessionDocStatus(SessionDocStatus::eActive);
  } else {
    SetMediaSessionDocStatus(SessionDocStatus::eInactive);
  }
}

void MediaSession::SetMediaSessionDocStatus(SessionDocStatus aState) {
  if (mSessionDocState == aState) {
    return;
  }
  mSessionDocState = aState;
  NotifyMediaSessionDocStatus(mSessionDocState);
}

nsPIDOMWindowInner* MediaSession::GetParentObject() const { return mParent; }

JSObject* MediaSession::WrapObject(JSContext* aCx,
                                   JS::Handle<JSObject*> aGivenProto) {
  return MediaSession_Binding::Wrap(aCx, this, aGivenProto);
}

MediaMetadata* MediaSession::GetMetadata() const { return mMediaMetadata; }

void MediaSession::SetMetadata(MediaMetadata* aMetadata) {
  MOZ_ASSERT(mSessionDocState == SessionDocStatus::eActive);
  mMediaMetadata = aMetadata;
  NotifyMetadataUpdated();
}

void MediaSession::SetPlaybackState(
    const MediaSessionPlaybackState& aPlaybackState) {
  MOZ_ASSERT(mSessionDocState == SessionDocStatus::eActive);
  if (mDeclaredPlaybackState == aPlaybackState) {
    return;
  }
  mDeclaredPlaybackState = aPlaybackState;

  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC,
             "Update session playback state after context destroyed!");
  if (RefPtr<IMediaInfoUpdater> updater = ContentMediaAgent::Get(currentBC)) {
    updater->SetDeclaredPlaybackState(currentBC->Id(), mDeclaredPlaybackState);
  }
}

MediaSessionPlaybackState MediaSession::PlaybackState() const {
  return mDeclaredPlaybackState;
}

void MediaSession::SetActionHandler(MediaSessionAction aAction,
                                    MediaSessionActionHandler* aHandler) {
  MOZ_ASSERT(mSessionDocState == SessionDocStatus::eActive);
  MOZ_ASSERT(size_t(aAction) < ArrayLength(mActionHandlers));
  // If the media session changes its supported action, then we would propagate
  // this information to the chrome process in order to run the media session
  // actions update algorithm.
  // https://w3c.github.io/mediasession/#supported-media-session-actions
  RefPtr<MediaSessionActionHandler>& hanlder = mActionHandlers[aAction];
  if (!hanlder && aHandler) {
    NotifyEnableSupportedAction(aAction);
  } else if (hanlder && !aHandler) {
    NotifyDisableSupportedAction(aAction);
  }
  mActionHandlers[aAction] = aHandler;
}

MediaSessionActionHandler* MediaSession::GetActionHandler(
    MediaSessionAction aAction) const {
  MOZ_ASSERT(size_t(aAction) < ArrayLength(mActionHandlers));
  return mActionHandlers[aAction];
}

void MediaSession::SetPositionState(const MediaPositionState& aState,
                                    ErrorResult& aRv) {
  MOZ_ASSERT(mSessionDocState == SessionDocStatus::eActive);
  // https://w3c.github.io/mediasession/#dom-mediasession-setpositionstate
  // If the state is an empty dictionary then clear the position state.
  if (!aState.IsAnyMemberPresent()) {
    mPositionState.reset();
    return;
  }

  // If the duration is not present, throw a TypeError.
  if (!aState.mDuration.WasPassed()) {
    return aRv.ThrowTypeError("Duration is not present");
  }

  // If the duration is negative, throw a TypeError.
  if (aState.mDuration.WasPassed() && aState.mDuration.Value() < 0.0) {
    return aRv.ThrowTypeError(nsPrintfCString(
        "Invalid duration %f, it can't be negative", aState.mDuration.Value()));
  }

  // If the position is negative or greater than duration, throw a TypeError.
  if (aState.mPosition.WasPassed() &&
      (aState.mPosition.Value() < 0.0 ||
       aState.mPosition.Value() > aState.mDuration.Value())) {
    return aRv.ThrowTypeError(nsPrintfCString(
        "Invalid position %f, it can't be negative or greater than duration",
        aState.mPosition.Value()));
  }

  // If the playbackRate is zero, throw a TypeError.
  if (aState.mPlaybackRate.WasPassed() && aState.mPlaybackRate.Value() == 0.0) {
    return aRv.ThrowTypeError("The playbackRate is zero");
  }

  // If the position is not present, set it to zero.
  double position = aState.mPosition.WasPassed() ? aState.mPosition.Value() : 0;

  // If the playbackRate is not present, set it to 1.0.
  double playbackRate =
      aState.mPlaybackRate.WasPassed() ? aState.mPlaybackRate.Value() : 1.0;

  // Update the position state and last position updated time.
  MOZ_ASSERT(aState.mDuration.WasPassed());
  mPositionState =
      Some(PositionState(aState.mDuration.Value(), playbackRate, position));
  NotifyPositionStateChanged();
}

void MediaSession::NotifyHandler(const MediaSessionActionDetails& aDetails) {
  DispatchNotifyHandler(aDetails);
}

void MediaSession::DispatchNotifyHandler(
    const MediaSessionActionDetails& aDetails) {
  class Runnable final : public mozilla::Runnable {
   public:
    Runnable(const MediaSession* aSession,
             const MediaSessionActionDetails& aDetails)
        : mozilla::Runnable("MediaSession::DispatchNotifyHandler"),
          mSession(aSession),
          mDetails(aDetails) {}

    MOZ_CAN_RUN_SCRIPT_BOUNDARY NS_IMETHOD Run() override {
      if (RefPtr<MediaSessionActionHandler> handler =
              mSession->GetActionHandler(mDetails.mAction)) {
        handler->Call(mDetails);
      }
      return NS_OK;
    }

   private:
    RefPtr<const MediaSession> mSession;
    MediaSessionActionDetails mDetails;
  };

  RefPtr<nsIRunnable> runnable = new Runnable(this, aDetails);
  NS_DispatchToMainThread(runnable);
}

bool MediaSession::IsSupportedAction(MediaSessionAction aAction) const {
  MOZ_ASSERT(size_t(aAction) < ArrayLength(mActionHandlers));
  return mActionHandlers[aAction] != nullptr;
}

bool MediaSession::IsActive() const {
  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC);
  RefPtr<WindowContext> wc = currentBC->GetTopWindowContext();
  if (!wc) {
    return false;
  }
  Maybe<uint64_t> activeSessionContextId = wc->GetActiveMediaSessionContextId();
  if (!activeSessionContextId) {
    return false;
  }
  LOG("session context Id=%" PRIu64 ", active session context Id=%" PRIu64,
      currentBC->Id(), *activeSessionContextId);
  return *activeSessionContextId == currentBC->Id();
}

void MediaSession::NotifyMediaSessionDocStatus(SessionDocStatus aState) {
  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC, "Update session status after context destroyed!");

  RefPtr<IMediaInfoUpdater> updater = ContentMediaAgent::Get(currentBC);
  if (!updater) {
    return;
  }
  if (aState == SessionDocStatus::eActive) {
    updater->NotifySessionCreated(currentBC->Id());
  } else {
    updater->NotifySessionDestroyed(currentBC->Id());
  }
}

void MediaSession::NotifyMetadataUpdated() {
  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC, "Update session metadata after context destroyed!");

  Maybe<MediaMetadataBase> metadata;
  if (GetMetadata()) {
    metadata.emplace(*(GetMetadata()->AsMetadataBase()));
  }
  if (RefPtr<IMediaInfoUpdater> updater = ContentMediaAgent::Get(currentBC)) {
    updater->UpdateMetadata(currentBC->Id(), metadata);
  }
}

void MediaSession::NotifyEnableSupportedAction(MediaSessionAction aAction) {
  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC, "Update action after context destroyed!");
  if (RefPtr<IMediaInfoUpdater> updater = ContentMediaAgent::Get(currentBC)) {
    updater->EnableAction(currentBC->Id(), aAction);
  }
}

void MediaSession::NotifyDisableSupportedAction(MediaSessionAction aAction) {
  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC, "Update action after context destroyed!");
  if (RefPtr<IMediaInfoUpdater> updater = ContentMediaAgent::Get(currentBC)) {
    updater->DisableAction(currentBC->Id(), aAction);
  }
}

void MediaSession::NotifyPositionStateChanged() {
  RefPtr<BrowsingContext> currentBC = GetParentObject()->GetBrowsingContext();
  MOZ_ASSERT(currentBC, "Update action after context destroyed!");
  if (RefPtr<IMediaInfoUpdater> updater = ContentMediaAgent::Get(currentBC)) {
    updater->UpdatePositionState(currentBC->Id(), *mPositionState);
  }
}

}  // namespace dom
}  // namespace mozilla
