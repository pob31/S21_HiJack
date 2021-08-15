/*
  ==============================================================================

    GraphicEq.cpp
    Created: 14 Aug 2021 4:07:14pm
    Author:  Pierre-Olivier

  ==============================================================================
*/

#include "GraphicEq.h"

GeqGain::GeqGain(std::string oscMtd)
{
    oscMethod = oscMtd + oscMethod;
}

GeqGain::~GeqGain()
{
}

void GeqGain::updateOSCmethod(std::string osdMtd)
{
    geqGain.updateOSCmethod(osdMtd + oscMethod);
}

GeqIn::GeqIn(std::string oscMtd)
{
    oscMethod =  oscMtd + oscMethod;
}

GeqIn::~GeqIn()
{
}

void GeqIn::updateOSCmethod(std::string osdMtd)
{
    geqIn.updateOSCmethod(osdMtd + oscMethod);
}

GraphicEq::GraphicEq(int geqID)
{
    oscMethod = "/Graphic_Eq/" + std::to_string(geqID);
    geqGain.updateOSCmethod(oscMethod);
    geqIn.updateOSCmethod(oscMethod);
}

GraphicEq::~GraphicEq()
{
}