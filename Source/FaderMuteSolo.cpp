/*
  ==============================================================================

    FaderMuteSolo.cpp
    Created: 13 Aug 2021 11:30:16pm
    Author:  Pierre-Olivier

  ==============================================================================
*/

#include "FaderMuteSolo.h"

Fader::Fader(std::string oscMtd)
{
    oscMethod = oscMtd + oscMethod;
}

Fader::~Fader()
{
}

void Fader::updateOSCmethod(std::string osdMtd)
{
    fader.updateOSCmethod(osdMtd + oscMethod);
}

Mute::Mute(std::string oscMtd)
{
    oscMethod =  oscMtd + oscMethod;
}

Mute::~Mute()
{
}

void Mute::updateOSCmethod(std::string osdMtd)
{
    mute.updateOSCmethod(osdMtd + oscMethod);
}

Solo::Solo(std::string oscMtd)
{
    oscMethod = oscMtd + oscMethod;
}

Solo::~Solo()
{
}

void Solo::updateOSCmethod(std::string osdMtd)
{
    solo.updateOSCmethod(osdMtd + oscMethod);
}