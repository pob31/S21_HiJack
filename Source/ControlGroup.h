/*
  ==============================================================================

    ControlGroup.h
    Created: 14 Aug 2021 11:41:22am
    Author:  Pierre-Olivier

  ==============================================================================
*/

#pragma once
#include "FaderMuteSolo.h"
#include <string>

class ControlGroup
{
public:
    ControlGroup (int channelID);
    ~ControlGroup();

private:
    std::string oscMethod;

    Fader fader{ oscMethod };
    Mute mute{ oscMethod };
    Solo solo{ oscMethod };
    
};

