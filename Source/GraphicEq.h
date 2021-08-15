/*
  ==============================================================================

    GraphicEq.h
    Created: 14 Aug 2021 4:07:14pm
    Author:  Pierre-Olivier

  ==============================================================================
*/

#pragma once
#include <iostream>
#include <string>
#include "OSCvalues.h"

class GeqGain
{
public:
    std::string oscMethod{ "/geq_gain" };

    GeqGain(std::string oscMtd);
    ~GeqGain();

    void updateOSCmethod(std::string osdMtd);

private:
    OSCfloat geqGain{ oscMethod, -12, 12, 0 };
    
};

class GeqIn
{
public:
    GeqIn(std::string oscMtd);
    ~GeqIn();

    void updateOSCmethod(std::string osdMtd);

private:
    std::string oscMethod = "/geq_in";
    OSCwholeFloat geqIn{ oscMethod, 0, 1, 0 };
    
};

Class GraphicEq
{
public:
    GraphicEq (int geqID);
    ~GraphicEq();

private:
    std::string oscMethod;

    GeqGain geqGain{ oscMethod };
    GeqIn geqIn{ oscMethod };
    
};

private:

};