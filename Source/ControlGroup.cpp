/*
  ==============================================================================

    ControlGroup.cpp
    Created: 14 Aug 2021 11:41:22am
    Author:  Pierre-Olivier

  ==============================================================================
*/

#include "ControlGroup.h"

ControlGroup::ControlGroup(int channelID)
{
    oscMethod = "/Control_Groups/" + std::to_string(channelID);
    fader.updateOSCmethod(oscMethod);
    mute.updateOSCmethod(oscMethod);
    solo.updateOSCmethod(oscMethod);
}

ControlGroup::~ControlGroup()
{
}