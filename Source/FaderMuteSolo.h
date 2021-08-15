/*
  ==============================================================================

    FaderMuteSolo.h
    Created: 13 Aug 2021 11:30:16pm
    Author:  Pierre-Olivier

  ==============================================================================
*/

#pragma once
#include <iostream>
#include <string>
#include "OSCvalues.h"

class Fader
{
public:
    std::string oscMethod{ "/fader" };

    Fader(std::string oscMtd);
    ~Fader();

    void updateOSCmethod(std::string osdMtd);

private:
    OSCfloat fader{ oscMethod, -120, 10, -120 };
};

class Mute
{
public:
    Mute(std::string oscMtd);
    ~Mute();

    void updateOSCmethod(std::string osdMtd);

private:
    std::string oscMethod = "/mute";
    OSCwholeFloat mute{ oscMethod, 0, 1, 0 };
};

class Solo
{
public:
    Solo(std::string oscMtd);
    ~Solo();

    void updateOSCmethod(std::string osdMtd);

private:
    std::string oscMethod = "/solo";
    OSCwholeFloat solo{oscMethod, 0, 1, 0};
};


